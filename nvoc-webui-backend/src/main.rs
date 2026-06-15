use axum::{
    Json, Router,
    extract::{Path, Query},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use nvoc_core::{
    BackendSet, Celsius, ClockDomain, ConvertEnum, GpuSelector, GpuTarget, Kilohertz,
    KilohertzDelta, Microvolts, NvapiLockedVoltageTarget, PState, Percentage, QueryDomainVfpPoints,
    QueryFanInfo, QueryGpuInfo, QueryGpuSettings, QueryGpuStatus, QueryLegacyCoreOvervoltRanges,
    QueryPowerLimits, QueryPstates, QueryTemperatureThresholds, ResetCoolerLevels,
    ResetLockedClocks, ResetNvapiPowerLimits, ResetNvapiSensorLimits, ResetPstateBaseVoltages,
    ResetPstateClockOffsets, ResetVfpDeltas, ResetVfpFrequencyLock, ResetVfpLock, SetClockOffset,
    SetCoolerLevels, SetFanSpeed, SetLockedClocks, SetNvapiPowerLimits, SetNvapiPstateLock,
    SetNvapiSensorLimits, SetNvmlPstateLock, SetPowerLimit, SetPstateClockOffset,
    SetVfpFrequencyLock, SetVfpRangeDelta, SetVfpVoltageLock, SetVoltageBoost, TargetInventory,
    VfpResetDomain, discover_targets, nvml_pstate_to_str, parse_nvml_fan_control_policy, run,
    select_targets, try_parse_nvml_pstate,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};
use std::{env, net::SocketAddr};
use utoipa::{IntoParams, OpenApi, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

type ApiResult<T> = Result<Json<T>, ApiError>;

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(value_object([
            ("ok", Value::Bool(false)),
            ("error", Value::String(self.message)),
        ]));
        (self.status, body).into_response()
    }
}

fn to_api_err(err: impl std::fmt::Display) -> ApiError {
    ApiError::internal(err.to_string())
}

#[derive(Debug, Deserialize, IntoParams)]
struct BackendQuery {
    backend: Option<String>,
    backends: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
struct VfpQuery {
    domain: Option<String>,
    infer_missing_default: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
struct OverclockRequest {
    backend: Option<String>,
    core_offset_khz: i32,
    mem_offset_khz: i32,
    pstate: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
struct BackendRequest {
    backend: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
struct LimitsRequest {
    backend: Option<String>,
    power_limit: Option<u32>,
    thermal_limit: Option<i32>,
    voltage_boost: Option<u32>,
}

#[derive(Debug, Deserialize, ToSchema)]
struct FanRequest {
    backend: Option<String>,
    fan_id: Option<String>,
    policy: Option<String>,
    level: Option<u32>,
    reset: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
struct PstateLockRequest {
    backend: Option<String>,
    first_pstate: String,
    second_pstate: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
struct VfpRangeDeltaRequest {
    start: usize,
    end: usize,
    delta_khz: i32,
}

#[derive(Debug, Deserialize, ToSchema)]
struct VfpVoltageLockRequest {
    point: Option<usize>,
    voltage_uv: Option<u32>,
    feedback: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
struct VfpFrequencyLockRequest {
    backend: Option<String>,
    domain: Option<String>,
    min_khz: u32,
    max_khz: u32,
}

#[derive(Debug, Deserialize, ToSchema)]
struct VfpDomainRequest {
    backend: Option<String>,
    domain: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
struct ActionResponse {
    ok: bool,
    message: String,
}

#[derive(Debug, Serialize, ToSchema)]
struct ErrorResponse {
    ok: bool,
    error: String,
}

#[derive(OpenApi)]
#[openapi(
    components(schemas(
        ActionResponse,
        BackendRequest,
        ErrorResponse,
        FanRequest,
        LimitsRequest,
        OverclockRequest,
        PstateLockRequest,
        VfpDomainRequest,
        VfpFrequencyLockRequest,
        VfpRangeDeltaRequest,
        VfpVoltageLockRequest,
    )),
    tags(
        (name = "system", description = "Backend health and discovery endpoints"),
        (name = "gpu", description = "Read-only GPU inventory and telemetry endpoints"),
        (name = "control", description = "GPU overclocking and limit mutation endpoints"),
        (name = "vfcurve", description = "Voltage/frequency curve endpoints")
    )
)]
struct ApiDoc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = env::var("NVOC_WEBUI_BIND")
        .unwrap_or_else(|_| "127.0.0.1:14515".to_string())
        .parse::<SocketAddr>()?;

    let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(root))
        .routes(routes!(health))
        .routes(routes!(list_gpus))
        .routes(routes!(gpu_info))
        .routes(routes!(gpu_status))
        .routes(routes!(gpu_settings))
        .routes(routes!(gpu_vfcurve))
        .routes(routes!(apply_overclock))
        .routes(routes!(reset_overclock))
        .routes(routes!(apply_limits))
        .routes(routes!(reset_limits))
        .routes(routes!(apply_fan))
        .routes(routes!(apply_pstate_lock))
        .routes(routes!(reset_pstate_lock))
        .routes(routes!(vfp_range_delta))
        .routes(routes!(vfp_voltage_lock))
        .routes(routes!(vfp_frequency_lock))
        .routes(routes!(reset_vfp_frequency_lock))
        .routes(routes!(reset_vfp_deltas))
        .routes(routes!(reset_vfp_lock))
        .split_for_parts();

    let app = router
        .route("/health", get(health))
        .merge(openapi_routes(api));

    println!("NVOC WebUI backend listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn openapi_routes(api: utoipa::openapi::OpenApi) -> Router {
    Router::new().route(
        "/api-docs/openapi.json",
        get(move || {
            let api = api.clone();
            async move { Json(api) }
        }),
    )
}

#[utoipa::path(
    get,
    path = "/",
    tag = "system",
    responses((status = 200, description = "Backend metadata"))
)]
async fn root() -> Json<Value> {
    Json(value_object([
        ("name", Value::String("nvoc-webui-backend".to_string())),
        ("ok", Value::Bool(true)),
        (
            "api",
            Value::Array(vec![
                Value::String("/api/gpus".to_string()),
                Value::String("/api/gpus/{gpu}/status".to_string()),
                Value::String("/api/gpus/{gpu}/settings".to_string()),
            ]),
        ),
    ]))
}

#[utoipa::path(
    get,
    path = "/api/health",
    tag = "system",
    responses((status = 200, description = "Backend health"))
)]
async fn health() -> Json<Value> {
    Json(value_object([("ok", Value::Bool(true))]))
}

#[utoipa::path(
    get,
    path = "/api/gpus",
    tag = "gpu",
    params(BackendQuery),
    responses(
        (status = 200, description = "Discovered GPU list"),
        (status = 500, description = "Driver discovery failed", body = ErrorResponse)
    )
)]
async fn list_gpus(Query(query): Query<BackendQuery>) -> ApiResult<Value> {
    let inventory = target_inventory(parse_backends(
        query
            .backends
            .as_deref()
            .or(query.backend.as_deref())
            .unwrap_or("both"),
    )?)?;
    let mut items = Vec::new();
    for target in inventory.targets() {
        let mut item = Map::new();
        item.insert("index".into(), u64_value(target.index as u64));
        item.insert("gpu_id".into(), u64_value(target.id.0 as u64));
        item.insert("gpu_id_hex".into(), text(format!("0x{:04X}", target.id.0)));
        item.insert("backend_nvapi".into(), bool_value(target.has_nvapi()));
        item.insert("backend_nvml".into(), bool_value(target.has_nvml()));
        if let Ok(info) = run(&target, QueryGpuInfo).map(|report| report.output) {
            item.insert("name".into(), text(info.name));
            item.insert("codename".into(), text(info.codename));
            item.insert("arch".into(), text(info.arch));
        }
        items.push(Value::Object(item));
    }
    Ok(Json(Value::Array(items)))
}

#[utoipa::path(
    get,
    path = "/api/gpus/{gpu}/info",
    tag = "gpu",
    params(
        ("gpu" = String, Path, description = "GPU index, decimal id, or hex id"),
        BackendQuery
    ),
    responses(
        (status = 200, description = "Normalized GPU information"),
        (status = 400, description = "Invalid GPU selector", body = ErrorResponse),
        (status = 500, description = "Driver query failed", body = ErrorResponse)
    )
)]
async fn gpu_info(Path(gpu): Path<String>, Query(query): Query<BackendQuery>) -> ApiResult<Value> {
    with_target_json(
        &gpu,
        query.backends.as_deref().unwrap_or("both"),
        normalize_info,
    )
}

#[utoipa::path(
    get,
    path = "/api/gpus/{gpu}/status",
    tag = "gpu",
    params(
        ("gpu" = String, Path, description = "GPU index, decimal id, or hex id"),
        BackendQuery
    ),
    responses(
        (status = 200, description = "Normalized live GPU telemetry"),
        (status = 400, description = "Invalid GPU selector", body = ErrorResponse),
        (status = 500, description = "Driver query failed", body = ErrorResponse)
    )
)]
async fn gpu_status(
    Path(gpu): Path<String>,
    Query(query): Query<BackendQuery>,
) -> ApiResult<Value> {
    with_target_json(
        &gpu,
        query.backends.as_deref().unwrap_or("both"),
        normalize_status,
    )
}

#[utoipa::path(
    get,
    path = "/api/gpus/{gpu}/settings",
    tag = "gpu",
    params(
        ("gpu" = String, Path, description = "GPU index, decimal id, or hex id"),
        BackendQuery
    ),
    responses(
        (status = 200, description = "Normalized current GPU settings"),
        (status = 400, description = "Invalid GPU selector", body = ErrorResponse),
        (status = 500, description = "Driver query failed", body = ErrorResponse)
    )
)]
async fn gpu_settings(
    Path(gpu): Path<String>,
    Query(query): Query<BackendQuery>,
) -> ApiResult<Value> {
    with_target_json(
        &gpu,
        query.backends.as_deref().unwrap_or("both"),
        normalize_settings,
    )
}

#[utoipa::path(
    get,
    path = "/api/gpus/{gpu}/vfcurve",
    tag = "vfcurve",
    params(
        ("gpu" = String, Path, description = "GPU index, decimal id, or hex id"),
        VfpQuery
    ),
    responses(
        (status = 200, description = "Indexed voltage/frequency curve points"),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 500, description = "Driver query failed", body = ErrorResponse)
    )
)]
async fn gpu_vfcurve(Path(gpu): Path<String>, Query(query): Query<VfpQuery>) -> ApiResult<Value> {
    let domain = parse_domain(query.domain.as_deref().unwrap_or("graphics"))?;
    with_target_json(&gpu, "nvapi", |target| {
        normalize_domain_vfp_points(target, domain, query.infer_missing_default.unwrap_or(true))
    })
}

#[utoipa::path(
    post,
    path = "/api/gpus/{gpu}/overclock",
    tag = "control",
    request_body = OverclockRequest,
    params(("gpu" = String, Path, description = "GPU index, decimal id, or hex id")),
    responses(
        (status = 200, description = "Overclock offsets applied", body = ActionResponse),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 500, description = "Driver mutation failed", body = ErrorResponse)
    )
)]
async fn apply_overclock(
    Path(gpu): Path<String>,
    Json(request): Json<OverclockRequest>,
) -> ApiResult<ActionResponse> {
    let backend = parse_backend(request.backend.as_deref().unwrap_or("nvapi"))?;
    let inventory = target_inventory(if backend == "nvml" {
        BackendSet::Nvml
    } else {
        BackendSet::Nvapi
    })?;
    let target = selected_target(&inventory, &gpu)?;
    match backend {
        "nvml" => {
            let pstate = try_parse_nvml_pstate(request.pstate.as_deref().unwrap_or("P0"))
                .map_err(|err| ApiError::bad_request(err.to_string()))?;
            run(
                &target,
                SetClockOffset {
                    domain: ClockDomain::Graphics,
                    pstate,
                    mhz: request.core_offset_khz / 1000,
                },
            )
            .map_err(to_api_err)?;
            run(
                &target,
                SetClockOffset {
                    domain: ClockDomain::Memory,
                    pstate,
                    mhz: request.mem_offset_khz / 1000,
                },
            )
            .map_err(to_api_err)?;
        }
        "nvapi" => {
            let pstate = parse_pstate(request.pstate.as_deref().unwrap_or("P0"))?;
            run(
                &target,
                SetPstateClockOffset {
                    pstate,
                    domain: ClockDomain::Graphics,
                    delta: KilohertzDelta(request.core_offset_khz),
                },
            )
            .map_err(to_api_err)?;
            run(
                &target,
                SetPstateClockOffset {
                    pstate,
                    domain: ClockDomain::Memory,
                    delta: KilohertzDelta(request.mem_offset_khz),
                },
            )
            .map_err(to_api_err)?;
        }
        _ => {
            return Err(ApiError::bad_request(
                "overclock backend must be nvapi or nvml",
            ));
        }
    }
    action_ok(format!("Applied {backend} overclock."))
}

#[utoipa::path(
    post,
    path = "/api/gpus/{gpu}/overclock/reset",
    tag = "control",
    request_body = BackendRequest,
    params(("gpu" = String, Path, description = "GPU index, decimal id, or hex id")),
    responses(
        (status = 200, description = "Overclock offsets reset", body = ActionResponse),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 500, description = "Driver mutation failed", body = ErrorResponse)
    )
)]
async fn reset_overclock(
    Path(gpu): Path<String>,
    Json(request): Json<BackendRequest>,
) -> ApiResult<ActionResponse> {
    let backend = parse_backend(request.backend.as_deref().unwrap_or("nvapi"))?;
    let inventory = target_inventory(if backend == "nvml" {
        BackendSet::Nvml
    } else {
        BackendSet::Nvapi
    })?;
    let target = selected_target(&inventory, &gpu)?;
    match backend {
        "nvml" => {
            let pstate = try_parse_nvml_pstate("P0")
                .map_err(|err| ApiError::bad_request(err.to_string()))?;
            for domain in [ClockDomain::Graphics, ClockDomain::Memory] {
                run(
                    &target,
                    SetClockOffset {
                        domain,
                        pstate,
                        mhz: 0,
                    },
                )
                .map_err(to_api_err)?;
            }
        }
        "nvapi" => {
            run(
                &target,
                ResetPstateClockOffsets {
                    offsets: vec![
                        (PState::P0, ClockDomain::Graphics),
                        (PState::P0, ClockDomain::Memory),
                    ],
                },
            )
            .map_err(to_api_err)?;
        }
        _ => {
            return Err(ApiError::bad_request(
                "clock reset backend must be nvapi or nvml",
            ));
        }
    }
    action_ok(format!("Reset {backend} overclock offsets."))
}

#[utoipa::path(
    post,
    path = "/api/gpus/{gpu}/limits",
    tag = "control",
    request_body = LimitsRequest,
    params(("gpu" = String, Path, description = "GPU index, decimal id, or hex id")),
    responses(
        (status = 200, description = "Limits applied", body = ActionResponse),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 500, description = "Driver mutation failed", body = ErrorResponse)
    )
)]
async fn apply_limits(
    Path(gpu): Path<String>,
    Json(request): Json<LimitsRequest>,
) -> ApiResult<ActionResponse> {
    let backend = parse_backend(request.backend.as_deref().unwrap_or("nvapi"))?;
    let inventory = target_inventory(if backend == "nvml" {
        BackendSet::Nvml
    } else {
        BackendSet::Nvapi
    })?;
    let target = selected_target(&inventory, &gpu)?;
    match backend {
        "nvml" => {
            if let Some(watts) = request.power_limit {
                run(&target, SetPowerLimit { watts }).map_err(to_api_err)?;
            }
            if let Some(celsius) = request.thermal_limit {
                run(&target, nvoc_core::SetTemperatureLimit { celsius }).map_err(to_api_err)?;
            }
        }
        "nvapi" => {
            if let Some(value) = request.power_limit {
                run(
                    &target,
                    SetNvapiPowerLimits {
                        limits: vec![Percentage(value)],
                    },
                )
                .map_err(to_api_err)?;
            }
            if let Some(value) = request.thermal_limit {
                run(
                    &target,
                    SetNvapiSensorLimits {
                        limits: vec![Celsius(value).into()],
                    },
                )
                .map_err(to_api_err)?;
            }
            if let Some(value) = request.voltage_boost {
                run(
                    &target,
                    SetVoltageBoost {
                        boost: Percentage(value),
                    },
                )
                .map_err(to_api_err)?;
            }
        }
        _ => {
            return Err(ApiError::bad_request(
                "limits backend must be nvapi or nvml",
            ));
        }
    }
    action_ok(format!("Applied {backend} limits."))
}

#[utoipa::path(
    post,
    path = "/api/gpus/{gpu}/limits/reset",
    tag = "control",
    params(("gpu" = String, Path, description = "GPU index, decimal id, or hex id")),
    responses(
        (status = 200, description = "Supported limits reset", body = ActionResponse),
        (status = 400, description = "Invalid GPU selector", body = ErrorResponse),
        (status = 500, description = "Driver mutation failed", body = ErrorResponse)
    )
)]
async fn reset_limits(Path(gpu): Path<String>) -> ApiResult<ActionResponse> {
    let inventory = target_inventory(BackendSet::Both)?;
    let target = selected_target(&inventory, &gpu)?;
    if target.has_nvapi() {
        run(
            &target,
            SetVoltageBoost {
                boost: Percentage(0),
            },
        )
        .map_err(to_api_err)?;
        run(&target, ResetNvapiSensorLimits).map_err(to_api_err)?;
        run(&target, ResetNvapiPowerLimits).map_err(to_api_err)?;
        run(&target, ResetCoolerLevels).map_err(to_api_err)?;
        run(
            &target,
            ResetVfpDeltas {
                domain: VfpResetDomain::All,
            },
        )
        .map_err(to_api_err)?;
        run(&target, ResetVfpLock).map_err(to_api_err)?;
        run(&target, ResetPstateBaseVoltages).map_err(to_api_err)?;
        run(
            &target,
            ResetPstateClockOffsets {
                offsets: vec![
                    (PState::P0, ClockDomain::Graphics),
                    (PState::P0, ClockDomain::Memory),
                ],
            },
        )
        .map_err(to_api_err)?;
    }
    if target.has_nvml() {
        let _ = run(
            &target,
            ResetLockedClocks {
                domain: ClockDomain::Graphics,
            },
        );
        let _ = run(
            &target,
            ResetLockedClocks {
                domain: ClockDomain::Memory,
            },
        );
    }
    action_ok("Reset all supported limits.".to_string())
}

#[utoipa::path(
    post,
    path = "/api/gpus/{gpu}/fan",
    tag = "control",
    request_body = FanRequest,
    params(("gpu" = String, Path, description = "GPU index, decimal id, or hex id")),
    responses(
        (status = 200, description = "Fan request applied", body = ActionResponse),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 500, description = "Driver mutation failed", body = ErrorResponse)
    )
)]
async fn apply_fan(
    Path(gpu): Path<String>,
    Json(request): Json<FanRequest>,
) -> ApiResult<ActionResponse> {
    let backend = parse_backend(request.backend.as_deref().unwrap_or("nvapi-cooler"))?;
    let fan_id = request.fan_id.as_deref().unwrap_or("all");
    let level = request.level.unwrap_or(60);
    let policy = request.policy.as_deref().unwrap_or("continuous");
    match backend {
        "nvml" | "nvml-cooler" => {
            let inventory = target_inventory(BackendSet::Nvml)?;
            let target = selected_target(&inventory, &gpu)?;
            let fan_count = run(&target, QueryFanInfo)
                .map(|report| report.output.count)
                .unwrap_or(1);
            let fan_indices = if fan_id == "all" {
                (0..fan_count).collect::<Vec<_>>()
            } else {
                vec![
                    fan_id
                        .parse::<u32>()
                        .map_err(|err| ApiError::bad_request(err.to_string()))?,
                ]
            };
            if request.reset.unwrap_or(false) {
                for fan_index in fan_indices {
                    run(&target, nvoc_core::ResetFanSpeed { fan_index }).map_err(to_api_err)?;
                }
            } else {
                let policy = parse_nvml_fan_control_policy(policy)
                    .map_err(|err| ApiError::bad_request(err.to_string()))?;
                for fan_index in fan_indices {
                    run(
                        &target,
                        SetFanSpeed {
                            fan_index,
                            policy,
                            level,
                        },
                    )
                    .map_err(to_api_err)?;
                }
            }
        }
        "nvapi" | "nvapi-cooler" => {
            let inventory = target_inventory(BackendSet::Nvapi)?;
            let target = selected_target(&inventory, &gpu)?;
            if request.reset.unwrap_or(false) {
                run(&target, ResetCoolerLevels).map_err(to_api_err)?;
            } else {
                let cooler_target = match fan_id {
                    "1" => nvoc_core::CoolerTarget::Cooler1,
                    "2" => nvoc_core::CoolerTarget::Cooler2,
                    _ => nvoc_core::CoolerTarget::All,
                };
                let mode = match policy.to_ascii_lowercase().as_str() {
                    "auto" | "continuous" => nvoc_core::CoolerPolicy::TemperatureContinuous,
                    "manual" => nvoc_core::CoolerPolicy::Manual,
                    other => nvoc_core::CoolerPolicy::from_str(other)
                        .map_err(|err| ApiError::bad_request(err.to_string()))?,
                };
                run(
                    &target,
                    SetCoolerLevels {
                        policy: mode,
                        level,
                        cooler_target,
                    },
                )
                .map_err(to_api_err)?;
            }
        }
        _ => {
            return Err(ApiError::bad_request(
                "fan backend must be nvapi-cooler or nvml-cooler",
            ));
        }
    }
    action_ok("Applied fan request.".to_string())
}

#[utoipa::path(
    post,
    path = "/api/gpus/{gpu}/pstate-lock",
    tag = "control",
    request_body = PstateLockRequest,
    params(("gpu" = String, Path, description = "GPU index, decimal id, or hex id")),
    responses(
        (status = 200, description = "PState lock applied", body = ActionResponse),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 500, description = "Driver mutation failed", body = ErrorResponse)
    )
)]
async fn apply_pstate_lock(
    Path(gpu): Path<String>,
    Json(request): Json<PstateLockRequest>,
) -> ApiResult<ActionResponse> {
    let backend = parse_backend(request.backend.as_deref().unwrap_or("nvapi"))?;
    let second = request
        .second_pstate
        .as_deref()
        .unwrap_or(request.first_pstate.as_str());
    let inventory = target_inventory(if backend == "nvml" {
        BackendSet::Nvml
    } else {
        BackendSet::Both
    })?;
    let target = selected_target(&inventory, &gpu)?;
    match backend {
        "nvml" => run(
            &target,
            SetNvmlPstateLock {
                first_pstate: try_parse_nvml_pstate(&request.first_pstate)
                    .map_err(|err| ApiError::bad_request(err.to_string()))?,
                second_pstate: try_parse_nvml_pstate(second)
                    .map_err(|err| ApiError::bad_request(err.to_string()))?,
            },
        )
        .map(|_| ())
        .map_err(to_api_err)?,
        "nvapi" => run(
            &target,
            SetNvapiPstateLock {
                first_pstate: try_parse_nvml_pstate(&request.first_pstate)
                    .map_err(|err| ApiError::bad_request(err.to_string()))?,
                second_pstate: try_parse_nvml_pstate(second)
                    .map_err(|err| ApiError::bad_request(err.to_string()))?,
            },
        )
        .map(|_| ())
        .map_err(to_api_err)?,
        _ => {
            return Err(ApiError::bad_request(
                "PState lock backend must be nvapi or nvml",
            ));
        }
    }
    action_ok(format!("Applied {backend} PState lock."))
}

#[utoipa::path(
    post,
    path = "/api/gpus/{gpu}/pstate-lock/reset",
    tag = "control",
    request_body = BackendRequest,
    params(("gpu" = String, Path, description = "GPU index, decimal id, or hex id")),
    responses(
        (status = 200, description = "PState lock reset", body = ActionResponse),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 500, description = "Driver mutation failed", body = ErrorResponse)
    )
)]
async fn reset_pstate_lock(
    Path(gpu): Path<String>,
    Json(request): Json<BackendRequest>,
) -> ApiResult<ActionResponse> {
    let backend = parse_backend(request.backend.as_deref().unwrap_or("nvapi"))?;
    let inventory = target_inventory(if backend == "nvml" {
        BackendSet::Nvml
    } else {
        BackendSet::Nvapi
    })?;
    let target = selected_target(&inventory, &gpu)?;
    if backend == "nvml" {
        run(
            &target,
            ResetLockedClocks {
                domain: ClockDomain::Memory,
            },
        )
        .map_err(to_api_err)?;
    } else {
        run(
            &target,
            ResetVfpFrequencyLock {
                domain: ClockDomain::Memory,
            },
        )
        .map_err(to_api_err)?;
    }
    action_ok(format!("Reset {backend} PState lock."))
}

#[utoipa::path(
    post,
    path = "/api/gpus/{gpu}/vfcurve/range-delta",
    tag = "vfcurve",
    request_body = VfpRangeDeltaRequest,
    params(("gpu" = String, Path, description = "GPU index, decimal id, or hex id")),
    responses(
        (status = 200, description = "VFP range delta applied", body = ActionResponse),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 500, description = "Driver mutation failed", body = ErrorResponse)
    )
)]
async fn vfp_range_delta(
    Path(gpu): Path<String>,
    Json(request): Json<VfpRangeDeltaRequest>,
) -> ApiResult<ActionResponse> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, &gpu)?;
    let (start, end) = if request.start <= request.end {
        (request.start, request.end)
    } else {
        (request.end, request.start)
    };
    run(
        &target,
        SetVfpRangeDelta {
            start,
            end,
            delta: KilohertzDelta(request.delta_khz),
        },
    )
    .map_err(to_api_err)?;
    action_ok(format!(
        "Applied {} kHz VFP delta to points {start}-{end}.",
        request.delta_khz
    ))
}

#[utoipa::path(
    post,
    path = "/api/gpus/{gpu}/vfcurve/voltage-lock",
    tag = "vfcurve",
    request_body = VfpVoltageLockRequest,
    params(("gpu" = String, Path, description = "GPU index, decimal id, or hex id")),
    responses(
        (status = 200, description = "VFP voltage lock applied", body = ActionResponse),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 500, description = "Driver mutation failed", body = ErrorResponse)
    )
)]
async fn vfp_voltage_lock(
    Path(gpu): Path<String>,
    Json(request): Json<VfpVoltageLockRequest>,
) -> ApiResult<ActionResponse> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, &gpu)?;
    let voltage_target = if let Some(point) = request.point {
        NvapiLockedVoltageTarget::Point(point)
    } else if let Some(voltage_uv) = request.voltage_uv {
        NvapiLockedVoltageTarget::Voltage(Microvolts(voltage_uv))
    } else {
        return Err(ApiError::bad_request("expected either point or voltage_uv"));
    };
    run(
        &target,
        SetVfpVoltageLock {
            voltage_target,
            feedback: request.feedback.unwrap_or(false),
        },
    )
    .map_err(to_api_err)?;
    action_ok("Applied VFP voltage lock.".to_string())
}

#[utoipa::path(
    post,
    path = "/api/gpus/{gpu}/vfcurve/frequency-lock",
    tag = "vfcurve",
    request_body = VfpFrequencyLockRequest,
    params(("gpu" = String, Path, description = "GPU index, decimal id, or hex id")),
    responses(
        (status = 200, description = "Frequency lock applied", body = ActionResponse),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 500, description = "Driver mutation failed", body = ErrorResponse)
    )
)]
async fn vfp_frequency_lock(
    Path(gpu): Path<String>,
    Json(request): Json<VfpFrequencyLockRequest>,
) -> ApiResult<ActionResponse> {
    let backend = parse_backend(request.backend.as_deref().unwrap_or("nvml"))?;
    let domain = parse_domain(request.domain.as_deref().unwrap_or("graphics"))?;
    let inventory = target_inventory(if backend == "nvml" {
        BackendSet::Nvml
    } else {
        BackendSet::Nvapi
    })?;
    let target = selected_target(&inventory, &gpu)?;
    if backend == "nvapi" {
        run(
            &target,
            SetVfpFrequencyLock {
                domain,
                upper: Kilohertz(request.max_khz),
                lower: Some(Kilohertz(request.min_khz)),
            },
        )
        .map_err(to_api_err)?;
    } else if backend == "nvml" {
        let min_mhz = request.min_khz / 1000;
        let max_mhz = request.max_khz / 1000;
        run(
            &target,
            SetLockedClocks {
                domain,
                min_mhz,
                max_mhz,
            },
        )
        .map_err(to_api_err)?;
    } else {
        return Err(ApiError::bad_request(
            "frequency lock backend must be nvapi or nvml",
        ));
    }
    action_ok(format!("Applied {backend} frequency lock."))
}

#[utoipa::path(
    post,
    path = "/api/gpus/{gpu}/vfcurve/frequency-lock/reset",
    tag = "vfcurve",
    request_body = VfpDomainRequest,
    params(("gpu" = String, Path, description = "GPU index, decimal id, or hex id")),
    responses(
        (status = 200, description = "Frequency lock reset", body = ActionResponse),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 500, description = "Driver mutation failed", body = ErrorResponse)
    )
)]
async fn reset_vfp_frequency_lock(
    Path(gpu): Path<String>,
    Json(request): Json<VfpDomainRequest>,
) -> ApiResult<ActionResponse> {
    let backend = parse_backend(request.backend.as_deref().unwrap_or("nvml"))?;
    let domain = parse_domain(request.domain.as_deref().unwrap_or("graphics"))?;
    let inventory = target_inventory(if backend == "nvml" {
        BackendSet::Nvml
    } else {
        BackendSet::Nvapi
    })?;
    let target = selected_target(&inventory, &gpu)?;
    if backend == "nvapi" {
        run(&target, ResetVfpFrequencyLock { domain }).map_err(to_api_err)?;
    } else if backend == "nvml" {
        run(&target, ResetLockedClocks { domain }).map_err(to_api_err)?;
    } else {
        return Err(ApiError::bad_request(
            "frequency reset backend must be nvapi or nvml",
        ));
    }
    action_ok(format!("Reset {backend} frequency lock."))
}

#[utoipa::path(
    post,
    path = "/api/gpus/{gpu}/vfcurve/deltas/reset",
    tag = "vfcurve",
    request_body = VfpDomainRequest,
    params(("gpu" = String, Path, description = "GPU index, decimal id, or hex id")),
    responses(
        (status = 200, description = "VFP deltas reset", body = ActionResponse),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 500, description = "Driver mutation failed", body = ErrorResponse)
    )
)]
async fn reset_vfp_deltas(
    Path(gpu): Path<String>,
    Json(request): Json<VfpDomainRequest>,
) -> ApiResult<ActionResponse> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, &gpu)?;
    run(
        &target,
        ResetVfpDeltas {
            domain: parse_vfp_reset_domain(request.domain.as_deref().unwrap_or("all"))?,
        },
    )
    .map_err(to_api_err)?;
    action_ok("Reset VFP deltas.".to_string())
}

#[utoipa::path(
    post,
    path = "/api/gpus/{gpu}/vfcurve/lock/reset",
    tag = "vfcurve",
    params(("gpu" = String, Path, description = "GPU index, decimal id, or hex id")),
    responses(
        (status = 200, description = "VFP voltage lock reset", body = ActionResponse),
        (status = 400, description = "Invalid GPU selector", body = ErrorResponse),
        (status = 500, description = "Driver mutation failed", body = ErrorResponse)
    )
)]
async fn reset_vfp_lock(Path(gpu): Path<String>) -> ApiResult<ActionResponse> {
    let inventory = target_inventory(BackendSet::Nvapi)?;
    let target = selected_target(&inventory, &gpu)?;
    run(&target, ResetVfpLock).map_err(to_api_err)?;
    action_ok("Reset VFP lock.".to_string())
}

fn parse_backends(raw: &str) -> Result<BackendSet, ApiError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "both" | "all" => Ok(BackendSet::Both),
        "nvapi" => Ok(BackendSet::Nvapi),
        "nvml" => Ok(BackendSet::Nvml),
        other => Err(ApiError::bad_request(format!(
            "invalid backend set {other:?}; expected both, nvapi, or nvml"
        ))),
    }
}

fn parse_backend(raw: &str) -> Result<&'static str, ApiError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "nvapi" => Ok("nvapi"),
        "nvml" => Ok("nvml"),
        "nvapi-cooler" => Ok("nvapi-cooler"),
        "nvml-cooler" => Ok("nvml-cooler"),
        other => Err(ApiError::bad_request(format!(
            "invalid backend {other:?}; expected nvapi, nvml, nvapi-cooler, or nvml-cooler"
        ))),
    }
}

fn parse_domain(raw: &str) -> Result<ClockDomain, ApiError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "core" | "gpu" | "graphics" => Ok(ClockDomain::Graphics),
        "mem" | "memory" => Ok(ClockDomain::Memory),
        other => Err(ApiError::bad_request(format!(
            "invalid clock domain {other:?}; expected graphics/core/gpu or memory/mem"
        ))),
    }
}

fn parse_pstate(raw: &str) -> Result<PState, ApiError> {
    PState::from_str(raw.trim().to_ascii_uppercase().as_str())
        .map_err(|err| ApiError::bad_request(err.to_string()))
}

fn parse_vfp_reset_domain(raw: &str) -> Result<VfpResetDomain, ApiError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "all" => Ok(VfpResetDomain::All),
        "core" | "graphics" => Ok(VfpResetDomain::Core),
        "memory" | "mem" => Ok(VfpResetDomain::Memory),
        other => Err(ApiError::bad_request(format!(
            "invalid VFP reset domain {other:?}"
        ))),
    }
}

fn target_inventory(backends: BackendSet) -> Result<TargetInventory, ApiError> {
    discover_targets(backends).map_err(to_api_err)
}

fn selected_target<'a>(
    inventory: &'a TargetInventory,
    gpu: &str,
) -> Result<GpuTarget<'a>, ApiError> {
    let selector = GpuSelector::from_specs([gpu.trim().to_string()]);
    let targets = inventory.targets();
    let selected = select_targets(&targets, &selector)
        .map_err(|err| ApiError::bad_request(err.to_string()))?;
    let selected_id = selected
        .first()
        .map(|target| target.id)
        .ok_or_else(|| ApiError::bad_request(format!("no GPU matches {gpu:?}")))?;
    inventory
        .target_by_id(selected_id)
        .map_err(|err| ApiError::bad_request(err.to_string()))
}

fn with_target_json<F>(gpu: &str, backends: &str, f: F) -> ApiResult<Value>
where
    F: FnOnce(&GpuTarget<'_>) -> Result<Value, ApiError>,
{
    let inventory = target_inventory(parse_backends(backends)?)?;
    let target = selected_target(&inventory, gpu)?;
    Ok(Json(f(&target)?))
}

fn normalize_info(target: &GpuTarget<'_>) -> Result<Value, ApiError> {
    let info = run(target, QueryGpuInfo).map_err(to_api_err)?.output;
    let mut map = Map::new();
    map.insert("gpu_id".into(), u64_value(target.id.0 as u64));
    map.insert("gpu_id_hex".into(), text(format!("0x{:04X}", target.id.0)));
    map.insert("index".into(), u64_value(target.index as u64));
    map.insert("name".into(), text(&info.name));
    map.insert("gpu_name".into(), text(&info.name));
    map.insert("codename".into(), text(&info.codename));
    map.insert("arch".into(), text(info.arch));
    map.insert("gpu_architecture".into(), text(info.arch));
    map.insert("gpu_type".into(), text(info.gpu_type));
    map.insert("bios_version".into(), text(&info.bios_version));
    map.insert("bus".into(), text(info.bus));
    if let Some(vendor) = info.vendor() {
        map.insert("vendor".into(), text(vendor));
    }

    for (clock, limit) in &info.vfp_limits {
        let key_prefix = match *clock {
            ClockDomain::Graphics => "core_clock",
            ClockDomain::Memory => "mem_clock",
            _ => continue,
        };
        map.insert(format!("{key_prefix}_range_display"), text(limit.range));
        map.insert(
            format!("{key_prefix}_min_khz"),
            i64_value(limit.range.min.0 as i64),
        );
        map.insert(
            format!("{key_prefix}_max_khz"),
            i64_value(limit.range.max.0 as i64),
        );
    }

    if let Some(limit) = info.power_limits.first() {
        map.insert(
            "power_limit_min".into(),
            u64_value(limit.range.min.0 as u64),
        );
        map.insert(
            "power_limit_max".into(),
            u64_value(limit.range.max.0 as u64),
        );
        map.insert(
            "power_limit_default".into(),
            u64_value(limit.default.0 as u64),
        );
    }
    if let Ok(power) = run(target, QueryPowerLimits).map(|report| report.output) {
        map.insert(
            "power_limit_nvml_min_w".into(),
            f64_value(power.min_watts as f64),
        );
        map.insert(
            "power_limit_nvml_current_w".into(),
            f64_value(power.current_watts as f64),
        );
        map.insert(
            "power_limit_nvml_max_w".into(),
            f64_value(power.max_watts as f64),
        );
        map.insert("power_watt_min".into(), f64_value(power.min_watts as f64));
        map.insert(
            "power_watt_current".into(),
            f64_value(power.current_watts as f64),
        );
        map.insert("power_watt_max".into(), f64_value(power.max_watts as f64));
    }
    if let Some(limit) = info.sensor_limits.first() {
        map.insert(
            "thermal_limit_min".into(),
            i64_value(limit.range.min.0 as i64),
        );
        map.insert(
            "thermal_limit_max".into(),
            i64_value(limit.range.max.0 as i64),
        );
        map.insert(
            "thermal_limit_default".into(),
            i64_value(limit.default.0 as i64),
        );
    }
    let overvolts = run(target, QueryLegacyCoreOvervoltRanges)
        .map(|report| report.output)
        .unwrap_or_default();
    if let Some((pstate, current, min, max)) = overvolts.first() {
        map.insert("legacy_overvolt_pstate".into(), text(pstate));
        map.insert(
            "legacy_overvolt_current_uv".into(),
            i64_value(current.0 as i64),
        );
        map.insert("legacy_overvolt_min_uv".into(), i64_value(min.0 as i64));
        map.insert("legacy_overvolt_max_uv".into(), i64_value(max.0 as i64));
    }
    Ok(Value::Object(map))
}

fn normalize_status(target: &GpuTarget<'_>) -> Result<Value, ApiError> {
    let status = run(target, QueryGpuStatus).map_err(to_api_err)?.output;
    let mut map = Map::new();
    map.insert("gpu_id".into(), u64_value(target.id.0 as u64));
    map.insert("gpu_id_hex".into(), text(format!("0x{:04X}", target.id.0)));
    map.insert("index".into(), u64_value(target.index as u64));
    map.insert("pstate".into(), text(status.pstate));
    if let Some(voltage) = status.voltage {
        map.insert("voltage_uv".into(), u64_value(voltage.0 as u64));
    }
    for (clock, freq) in &status.clocks {
        match *clock {
            ClockDomain::Graphics => {
                map.insert("gpu_clock_khz".into(), i64_value(freq.0 as i64));
            }
            ClockDomain::Memory => {
                map.insert("mem_clock_khz".into(), i64_value(freq.0 as i64));
            }
            _ => {}
        }
    }
    if let Some((_sensor, temp)) = status.sensors.first() {
        map.insert("temperature_c".into(), f64_value(temp.0 as f64));
    }
    if let Some((_channel, power)) = status.power.iter().next()
        && let Some(watts) = first_number_in_display(power)
    {
        map.insert("power_w".into(), f64_value(watts));
    }
    map.insert(
        "vfp_locked".into(),
        bool_value(!status.vfp_locks.is_empty()),
    );
    if let Some(lock) = status.vfp_locks.values().next() {
        map.insert("vfp_lock_display".into(), text(lock));
    }
    Ok(Value::Object(map))
}

fn normalize_settings(target: &GpuTarget<'_>) -> Result<Value, ApiError> {
    let settings = run(target, QueryGpuSettings).map_err(to_api_err)?.output;
    let mut map = Map::new();
    map.insert("gpu_id".into(), u64_value(target.id.0 as u64));
    map.insert("gpu_id_hex".into(), text(format!("0x{:04X}", target.id.0)));
    map.insert("index".into(), u64_value(target.index as u64));

    if let Some(boost) = settings.voltage_boost {
        map.insert("voltage_boost_current".into(), u64_value(boost.0 as u64));
    }
    if let Some(limit) = settings.power_limits.first() {
        map.insert("power_limit_current".into(), i64_value(limit.0 as i64));
    }
    if let Some(limit) = settings.sensor_limits.first() {
        map.insert(
            "thermal_limit_current".into(),
            i64_value(limit.value.0 as i64),
        );
    }
    for (pstate, clocks) in &settings.pstate_deltas {
        for (clock, delta) in clocks {
            if *pstate != PState::P0 {
                continue;
            }
            match *clock {
                ClockDomain::Graphics => {
                    map.insert("core_clock_current_khz".into(), i64_value(delta.0 as i64));
                }
                ClockDomain::Memory => {
                    map.insert("mem_clock_current_khz".into(), i64_value(delta.0 as i64));
                }
                _ => {}
            }
        }
    }

    if let Ok(pstates) = run(target, QueryPstates).map(|report| report.output) {
        let mut labels = Vec::new();
        let mut ranges = Vec::new();
        for item in pstates {
            let label = nvml_pstate_to_str(item.pstate).to_string();
            labels.push(Value::String(label.clone()));
            ranges.push(value_object([
                ("pstate", Value::String(label)),
                ("min_core_mhz", u64_value(item.min_core_mhz as u64)),
                ("max_core_mhz", u64_value(item.max_core_mhz as u64)),
                ("min_memory_mhz", u64_value(item.min_memory_mhz as u64)),
                ("max_memory_mhz", u64_value(item.max_memory_mhz as u64)),
            ]));
        }
        map.insert("supported_pstates".into(), Value::Array(labels));
        map.insert("pstate_ranges".into(), Value::Array(ranges));
    }
    if let Ok(power) = run(target, QueryPowerLimits).map(|report| report.output) {
        map.insert(
            "power_limit_nvml_min_w".into(),
            f64_value(power.min_watts as f64),
        );
        map.insert(
            "power_limit_nvml_current_w".into(),
            f64_value(power.current_watts as f64),
        );
        map.insert(
            "power_limit_nvml_max_w".into(),
            f64_value(power.max_watts as f64),
        );
    }
    if let Ok(fan) = run(target, QueryFanInfo).map(|report| report.output) {
        map.insert("fan_count".into(), u64_value(fan.count as u64));
        map.insert("fan_min".into(), option_u32(fan.min_speed));
        map.insert("fan_max".into(), option_u32(fan.max_speed));
    }
    if let Ok(thresholds) = run(target, QueryTemperatureThresholds).map(|report| report.output) {
        map.insert(
            "temperature_thresholds".into(),
            Value::Array(
                thresholds
                    .into_iter()
                    .map(|threshold| {
                        value_object([
                            ("name", Value::String(threshold.name.to_string())),
                            ("celsius", option_u32(threshold.celsius)),
                        ])
                    })
                    .collect(),
            ),
        );
    }
    let mut locks = Map::new();
    for (id, lock) in &settings.vfp_locks {
        if let Some(value) = lock.lock_value {
            locks.insert(id.to_string(), text(value));
        }
    }
    map.insert("vfp_locks".into(), Value::Object(locks));
    Ok(Value::Object(map))
}

fn normalize_domain_vfp_points(
    target: &GpuTarget<'_>,
    domain: ClockDomain,
    infer_missing_default: bool,
) -> Result<Value, ApiError> {
    let points = run(
        target,
        QueryDomainVfpPoints {
            domain,
            infer_missing_default,
            indexed: true,
        },
    )
    .map_err(to_api_err)?
    .output
    .into_iter()
    .map(|(index, point)| {
        value_object([
            ("index", u64_value(index as u64)),
            ("voltage_uv", u64_value(point.voltage.0 as u64)),
            ("frequency_khz", u64_value(point.frequency.0 as u64)),
            ("delta_khz", i64_value(point.delta.0 as i64)),
            (
                "default_frequency_khz",
                u64_value(point.default_frequency.0 as u64),
            ),
        ])
    })
    .collect();
    Ok(Value::Array(points))
}

fn action_ok(message: String) -> ApiResult<ActionResponse> {
    Ok(Json(ActionResponse { ok: true, message }))
}

fn value_object(entries: impl IntoIterator<Item = (impl Into<String>, Value)>) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        if !value.is_null() {
            map.insert(key.into(), value);
        }
    }
    Value::Object(map)
}

fn text<T: std::fmt::Display>(value: T) -> Value {
    Value::String(value.to_string())
}

fn i64_value(value: i64) -> Value {
    Value::Number(Number::from(value))
}

fn u64_value(value: u64) -> Value {
    Value::Number(Number::from(value))
}

fn f64_value(value: f64) -> Value {
    Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

fn option_u32(value: Option<u32>) -> Value {
    value.map(|v| u64_value(v as u64)).unwrap_or(Value::Null)
}

fn bool_value(value: bool) -> Value {
    Value::Bool(value)
}

fn first_number_in_display<T: std::fmt::Display>(value: T) -> Option<f64> {
    let rendered = value.to_string();
    let mut token = String::new();
    let mut started = false;
    for ch in rendered.chars() {
        if ch.is_ascii_digit() || ch == '-' || ch == '+' || ch == '.' {
            token.push(ch);
            started = true;
        } else if started {
            break;
        }
    }
    if token.is_empty() || token == "-" || token == "+" || token == "." {
        None
    } else {
        token.parse().ok()
    }
}
