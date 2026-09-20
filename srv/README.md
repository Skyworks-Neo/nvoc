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

## 1 Deploy (install / update / uninstall)

The service binary path must live outside the build output and the repository,
so the registration does not dangle when `target/` is cleaned or the repo is
deleted/moved. `srv/deploy.ps1` automates the whole cycle:

```powershell
# elevated PowerShell
cd srv
.\deploy.ps1                 # build (release) + stop + copy + install/start + health check
.\deploy.ps1 -SkipBuild      # reuse existing target\release binaries
.\deploy.ps1 -NoStart        # stage + install, leave service stopped
.\deploy.ps1 -InstallDir X   # override install dir (default D:\08-skyworks\nvoc-srv\install)
```

What it does: stop the service → copy `nvoc_service.exe`, the 6 service
helper exes and `nvoc-cli.exe` into the install dir → run
`install_service.exe` from there on first deploy (the registered binary path
is derived from install_service.exe's own directory) → start the service →
health-check `GET /version` and `GET /config`.

Manual equivalents:

- install: run `install_service.exe` **from the install dir** (it registers
  the `nvoc_service.exe` next to itself)
- uninstall: `uninstall_service.exe` (also removes the registration)
- update: stop service (`sc stop nvoc_service`), replace exes, start again
- check the state: `sc query nvoc_service`

Post-deploy acceptance (no elevation needed): `verify_deploy.ps1` checks the
service registration and install dir, that `/version` and the startup log
agree on the build identity, the `/config` shape, the CSRF/range guards on
both mutation endpoints, and one reversible `temp_limit` write that is
restored afterwards. Exits non-zero on any failure:

```powershell
.\verify_deploy.ps1                          # verify the live install
.\verify_deploy.ps1 -ExpectedGitHash <hash>  # also hard-assert the embedded hash
```

## 2 Check log

```
%PROGRAMDATA%\nvoc\logs\nvoc_service-output.log
```

The first log line is the build identity: `nvoc_service <version> (<git hash>) starting`.

## 3 Parameter update

A web service is on the 14514 port of localhost (loopback only).

Identify a deployed build:

```
curl.exe "http://127.0.0.1:14514/version"
{"version":"0.2.0-alpha.2","git_hash":"3f9c1a2b4d6e"}
```

To check the config

```
curl.exe "http://127.0.0.1:14514/config"
```

To change the temp limit to 44
```
curl.exe -X POST -H "X-Requested-With: XMLHttpRequest" "http://127.0.0.1:14514/set_temp_limit_soft_vfp?limit=44"
```

To set a global OC frequency
```
curl.exe -X POST -H "X-Requested-With: XMLHttpRequest" "http://127.0.0.1:14514/oc_global?oc=75"
```
