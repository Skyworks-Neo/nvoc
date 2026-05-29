# nvoc-cli

`nvoc-cli` is a focused command-line wrapper over `nvoc-core`.

## Usage

```text
nvoc-cli [--gpu GPU_ID] [--nvapi|--nvml] [--output human|json] <function-name> [args] [named args]
```

Named arguments can be placed before or after the function name. Use
`nvoc-cli <function-name> --help` to see the named arguments supported by a
specific function.

Examples:

```text
nvoc-cli get-vfp --gpu 0
nvoc-cli --domain memory get-vfp --output json
nvoc-cli --nvml get-power-watt
nvoc-cli set-core-offset-mhz 150 --gpu 0
nvoc-cli set-locked-clocks-mhz 210 2100 --domain core
```

When neither `--nvapi` nor `--nvml` is provided, commands that support both
backends try NVAPI first and fall back to NVML if the NVAPI attempt fails.
