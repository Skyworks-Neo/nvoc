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

`srv/deploy.ps1` automates the whole cycle: build → stop → install/start →
health check. By default the service is registered **from the build output
directory** — the same behavior as manually running `install_service.exe`
(the registered binary path is the `nvoc_service.exe` next to it, see
`srv/src/bin/install_service.rs`).

Whether the registration follows the build tree is now an explicit choice:
pass `-InstallDir` to stage the exes into a stable out-of-repo directory and
register from there instead. A registration pointing into the build output
dangles when that directory is deleted/moved (`cargo clean`, workspace
cleanup) — exactly how the old dangling `nvoc_service` registration happened —
so use `-InstallDir` if you routinely clean the build tree.

```powershell
# elevated PowerShell
cd srv
.\deploy.ps1                 # default: register from the build output dir
.\deploy.ps1 -InstallDir X   # stage exes into X and register from there
.\deploy.ps1 -SkipBuild      # reuse existing release binaries
.\deploy.ps1 -NoStart        # stage + install, leave service stopped
```

What it does: build (release) → stop the service → if `-InstallDir` differs
from the build output, copy `nvoc_service.exe`, the 6 service helper exes and
`nvoc-cli.exe` there → run `install_service.exe` from the install location on
first deploy, or re-register (`sc delete` + install) when an existing
registration points somewhere else → start the service → health-check
`GET /version` and `GET /config`.

Manual equivalents:

- install: run `install_service.exe` **from the install dir** (it registers
  the `nvoc_service.exe` next to itself)
- uninstall: `uninstall_service.exe` (also removes the registration)
- update: stop service (`sc stop nvoc_service`), replace exes, start again
- check the state: `sc query nvoc_service`

Post-deploy acceptance (no elevation needed): `verify_deploy.ps1` auto-detects
the service registration (the registered binary must exist on disk and sit
outside the repo), checks that `/version` and the startup log agree on the
build identity, the `/config` shape, the CSRF/range guards on both mutation
endpoints, and one reversible `temp_limit` write that is restored afterwards.
Pass `-InstallDir` only to additionally assert a specific registered location.
Exits non-zero on any failure:

```powershell
.\verify_deploy.ps1                          # verify the live install
.\verify_deploy.ps1 -ExpectedGitHash <hash>  # also hard-assert the embedded hash
.\verify_deploy.ps1 -InstallDir X            # also assert the registered location
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
