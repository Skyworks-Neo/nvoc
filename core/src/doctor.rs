//! NVAPI interface doctor — resolve the interface IDs nvoc depends on and
//! report availability, one `nvapi_QueryInterface` null-check per entry.
//!
//! Modeled on mVolt+ 0.50.5's self-inventory compat report (see
//! `docs/reverse-engineering/nvapi/mvolt-plus-nvapi-surface.md`): resolution
//! is never a call, so the probe is safe on any driver and answers "which of
//! the surfaces this build depends on did the driver stop exporting" after a
//! driver bump — the same interface-drift triage mVolt+ ships as its core
//! feature.
//!
//! The list below is curated, not exhaustive: every NVAPI write path nvoc
//! takes, plus the read families the GUI/TUI dashboards render. IDs are the
//! `nvid.rs` spellings; entries mVolt+ 0.50.5 also probes are marked so a
//! disagreement with its compat report on the same machine is easy to spot.

/// One interface probe result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NvapiDoctorEntry {
    /// Human-facing interface name (the `nvid.rs` spelling).
    pub name: &'static str,
    /// 32-bit QueryInterface selector.
    pub id: u32,
    /// `true` iff the loaded driver returned a handler for the ID.
    pub resolved: bool,
}

/// Full doctor report: interface generation + per-entry resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NvapiDoctorReport {
    /// `NvAPI_GetInterfaceVersionString` (e.g. "R580") — the driver-side API
    /// generation marker. `None` when the library failed to load/init.
    pub interface_version: Option<String>,
    /// Loader/init failure detail, when [`Self::interface_version`] is None.
    pub load_error: Option<String>,
    pub entries: Vec<NvapiDoctorEntry>,
}

impl NvapiDoctorReport {
    /// True when every curated entry resolved — a quick gate for callers.
    pub fn all_resolved(&self) -> bool {
        self.entries.iter().all(|e| e.resolved)
    }

    /// Names of the interfaces the loaded driver does NOT export.
    pub fn missing(&self) -> Vec<&'static str> {
        self.entries
            .iter()
            .filter(|e| !e.resolved)
            .map(|e| e.name)
            .collect()
    }
}

/// Curated (name, ID) probe list: nvoc's write paths + dashboard read
/// families. `(mVolt+)` marks interfaces mVolt+ 0.50.5 also resolves, so its
/// compat report doubles as a second opinion on the same machine.
static PROBES: &[(&str, u32)] = &[
    // --- init / enumeration -------------------------------------------------
    ("NvAPI_Initialize", 0x0150_E828),
    ("NvAPI_EnumPhysicalGPUs", 0xE5AC_921F),
    // --- GPU identity (mVolt+ plain table) ----------------------------------
    ("NvAPI_GPU_GetFullName (mVolt+)", 0xCEEE_8E9F),
    ("NvAPI_GPU_GetVbiosVersionString (mVolt+)", 0xA561_FD7D),
    ("NvAPI_GPU_GetPCIIdentifiers (mVolt+)", 0x2DDF_B66E),
    ("NvAPI_GPU_GetBusId (mVolt+)", 0x1BE0_B8E5),
    ("NvAPI_GPU_GetBusSlotId (mVolt+)", 0x2A0A_350F),
    ("NvAPI_GPU_GetArchInfo (mVolt+)", 0xD826_5D24),
    ("NvAPI_GPU_GetPhysicalFrameBufferSize (mVolt+)", 0x46FB_EB03),
    // --- pstates 2.0 (public + private delta table) --------------------------
    ("NvAPI_GPU_GetPstates20 (mVolt+)", 0x6FF8_1213),
    ("NvAPI_GPU_SetPstates20 (mVolt+)", 0x0F4D_AE6B),
    ("NvAPI_GPU_GetPstates20Private", 0xC5DD_F56E),
    ("NvAPI_GPU_SetPstates20Private", 0x4C0B_519A),
    ("NvAPI_GPU_SetForcePstate", 0x025B_FB10),
    ("NvAPI_GPU_GetCurrentPstate (mVolt+)", 0x927D_A4F6),
    // --- V/F curve / clock offsets (core OC surface) -------------------------
    (
        "NvAPI_GPU_ClockClientClkDomainsGetInfo (mVolt+)",
        0x64B4_3A6A,
    ),
    (
        "NvAPI_GPU_ClockClientClkVfPointsGetInfo (mVolt+)",
        0x507B_4B59,
    ),
    (
        "NvAPI_GPU_ClockClientClkVfPointsGetStatus (mVolt+)",
        0x2153_7AD4,
    ),
    (
        "NvAPI_GPU_ClockClientClkVfPointsGetControl (mVolt+)",
        0x23F1_B133,
    ),
    (
        "NvAPI_GPU_ClockClientClkVfPointsSetControl (mVolt+)",
        0x0733_E009,
    ),
    ("NvAPI_GPU_PerfClientLimitsGetStatus (mVolt+)", 0xE440_B867),
    ("NvAPI_GPU_PerfClientLimitsSetStatus (mVolt+)", 0x3944_2CFB),
    ("NvAPI_GPU_GetAllClocks (mVolt+)", 0x1BD6_9F49),
    ("NvAPI_GPU_ClockClkDomainsGetControl (mVolt+)", 0xF589_38F5),
    ("NvAPI_GPU_ClockClkDomainsSetControl (mVolt+)", 0xD14B_69CF),
    ("NvAPI_GPU_ClockClkDomainsMeasureFreq (mVolt+)", 0x527F_C458),
    // --- power / TGP ---------------------------------------------------------
    ("NvAPI_GPU_ClientPowerPoliciesGetInfo (mVolt+)", 0x3420_6D86),
    (
        "NvAPI_GPU_ClientPowerPoliciesGetStatus (mVolt+)",
        0x7091_6171,
    ),
    (
        "NvAPI_GPU_ClientPowerPoliciesSetStatus (mVolt+)",
        0xAD95_F5ED,
    ),
    ("NvAPI_GPU_ClientTgpWattGetStatus", 0x8B3E_7343),
    ("NvAPI_GPU_ClientTgpWattSetStatus", 0xAFFC_2279),
    (
        "NvAPI_GPU_ClientPowerTopologyGetStatus (mVolt+)",
        0xEDCF_624E,
    ),
    ("NvAPI_GPU_PowerMonitorGetInfo (mVolt+)", 0xC12E_B19E),
    ("NvAPI_GPU_PowerMonitorGetStatus (mVolt+)", 0xF402_38EF),
    // --- thermal -------------------------------------------------------------
    ("NvAPI_GPU_GetThermalSettings", 0xE364_0A56),
    ("NvAPI_GPU_ClientThermalPoliciesGetInfo", 0x0D25_8BB5),
    ("NvAPI_GPU_ClientThermalPoliciesGetStatus", 0xE9C4_25A1),
    ("NvAPI_GPU_ClientThermalPoliciesSetStatus", 0x34C0_B13D),
    ("NvAPI_GPU_GetPerfDecreaseInfo (mVolt+)", 0x7F7F_4600),
    // --- voltage rails / boost (melonVolt family) ----------------------------
    ("NvAPI_GPU_VoltVoltRailsGetInfo (mVolt+)", 0x2C73_AFDC),
    ("NvAPI_GPU_VoltVoltRailsGetControl (mVolt+)", 0xA307_0DB0),
    ("NvAPI_GPU_VoltVoltRailsGetStatus (mVolt+)", 0x5D06_34EE),
    ("NvAPI_GPU_VoltVoltRailsSetControl (mVolt+)", 0x87C5_5C8A),
    ("NvAPI_GPU_ClientVoltRailsGetControl", 0x9DF2_3CA1),
    ("NvAPI_GPU_ClientVoltRailsSetControl", 0xB930_6D9B),
    // --- fan -----------------------------------------------------------------
    ("NvAPI_GPU_ClientFanCoolersGetInfo (mVolt+)", 0xFB85_B01E),
    ("NvAPI_GPU_ClientFanCoolersGetStatus (mVolt+)", 0x35AE_D5E8),
    ("NvAPI_GPU_ClientFanCoolersGetControl (mVolt+)", 0x814B_209F),
    ("NvAPI_GPU_ClientFanCoolersSetControl (mVolt+)", 0xA589_71A5),
    // --- ADC sense devices (rail-voltage telemetry) ---------------------------
    ("NvAPI_GPU_ClockAdcDevicesGetInfo (mVolt+)", 0x6878_9E2A),
    ("NvAPI_GPU_ClockAdcDevicesGetStatus (mVolt+)", 0x43D9_B26A),
    // --- clock topology (research-grade; GET only in nvoc) --------------------
    ("NvAPI_GPU_ClockClkPropTopRelsGetInfo (mVolt+)", 0xE826_E4F0),
    (
        "NvAPI_GPU_ClockClkPropTopRelsGetControl (mVolt+)",
        0xCBFF_71D0,
    ),
];

/// Run the probe. Initializes NVAPI exactly once (the same explicit-init gate
/// every GPU call goes through), then resolves each curated ID. Never calls
/// any resolved handler, so this is safe on any driver, any GPU, and with no
/// GPU present at all.
pub fn nvapi_doctor_report() -> NvapiDoctorReport {
    let (interface_version, load_error) = match crate::nvapi_interface_version() {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(e.to_string())),
    };
    let entries = PROBES
        .iter()
        .map(|(name, id)| NvapiDoctorEntry {
            name,
            id: *id,
            resolved: ::nvapi::sys::nvapi::nvapi_QueryInterface(*id).is_ok(),
        })
        .collect();
    NvapiDoctorReport {
        interface_version,
        load_error,
        entries,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_ids_are_unique() {
        let mut ids: Vec<u32> = PROBES.iter().map(|(_, id)| *id).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate probe ID in doctor list");
    }

    #[test]
    fn probe_ids_sort_clean_hex() {
        // Every ID must render as a plain 32-bit selector (no sign bit set
        // beyond u32 range is fine, but nothing above it can exist by type).
        for (_, id) in PROBES {
            assert_eq!(format!("{id:08X}").len(), 8);
        }
    }
}
