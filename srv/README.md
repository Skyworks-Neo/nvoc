# NVOC-srv

Windows Service for Nvidia GPU Optimizer

## 0 Compile

clone this repository

├── auto-optimizer
├── srv

```
cargo build
```

p.s. Remember to stop the service when you compile and build the project.

## 1 Install and Uninstall service

### 1.1 install

```
.\target\debug\install_service.exe
```

### 1.2 uninstall

```
.\target\debug\uninstall_service.exe
```

### 1.3 check the state

```
sc query nvoc_service
```

## 2 Check log

On Windows the service writes logs under `%PROGRAMDATA%\nvoc\logs`, e.g.

```
C:\ProgramData\nvoc\logs\nvoc_service-output.log
```

## 3 Parameter update

A localhost-only HTTP control server listens on `127.0.0.1:14514` (loopback only;
it is not reachable from the network).

The service enforces a soft temperature wall: when a GPU's temperature reaches
`temp_limit` (°C) it steps the VFP voltage lock down toward a safer point, and
relaxes/unlocks again once the temperature is back to normal. Defaults:
`temp_limit = 60`, `vfp_lock_point = 70`.

State-changing endpoints are CSRF-guarded: they require `POST` **and** the header
`X-Requested-With: XMLHttpRequest`. A `GET` to them returns `405`.

Check the current config (read-only):
```
curl.exe "http://127.0.0.1:14514/config"
```

Set the soft temperature wall to 64 °C (valid range 40–120):
```
curl.exe -X POST -H "X-Requested-With: XMLHttpRequest" "http://127.0.0.1:14514/set_temp_limit_soft_vfp?limit=64"
```

Queue a global P0 graphics-clock offset on GPU 0 (`oc` is a kHz delta, range ±2000000; `gpu` defaults to 0):
```
curl.exe -X POST -H "X-Requested-With: XMLHttpRequest" "http://127.0.0.1:14514/oc_global?oc=75000&gpu=0"
```
