mkdir .\ws
if not defined NVOC_AUTO_OPTIMIZER_BIN set "NVOC_AUTO_OPTIMIZER_BIN=..\target\release\nvoc-auto-optimizer.exe"
if not defined NVOC_CLI_BIN set "NVOC_CLI_BIN=..\target\release\nvoc-cli.exe"

"%NVOC_CLI_BIN%" reset-core-offset-mhz
"%NVOC_CLI_BIN%" reset-memory-offset-mhz
"%NVOC_AUTO_OPTIMIZER_BIN%" import-vfp .\ws\vfp-init.csv
