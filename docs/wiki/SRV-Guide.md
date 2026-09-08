# SRV Guide

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

NVOC-SRV is the Windows Service and localhost HTTP control layer, targeting server, workstation, and managed machine scenarios.

### Building

```bash
cd srv
cargo build
```

> Stop any previously installed service before building.

### Service Install / Uninstall

#### Install

```bash
.\target\debug\install_service.exe
```

#### Uninstall

```bash
.\target\debug\uninstall_service.exe
```

#### Check Service Status

```bash
.\target\debug\uninstall_service.exe
```

### Logs

On Windows, logs are located at:

```
logs/
```

### HTTP API

The service provides HTTP endpoints at `localhost:1145` after starting.

#### View Configuration

```bash
curl.exe "http://127.0.0.1:1145/config"
```

#### Set Thermal Wall VFP Reference Point

```bash
# Set to point 44
curl.exe "http://127.0.0.1:1145/set_tem_wall_vfp?point=44"
```

When the thermal limit is exceeded, the GPU is set to the specified VFP reference point.

#### Set Global Overclock Frequency

```bash
# Set OC to 75
curl.exe "http://127.0.0.1:1145/oc_global?oc=75"
```

---

<a id="chinese"></a>

## 中文

NVOC-SRV 是 Windows Service 与 localhost HTTP 控制层，面向服务器、工作站和托管机器场景。

### 编译

```bash
cd srv
cargo build
```

> 编译前请先停止已安装的服务。

### 安装 / 卸载服务

#### 安装

```bash
.\target\debug\install_service.exe
```

#### 卸载

```bash
.\target\debug\uninstall_service.exe
```

#### 检查服务状态

```bash
.\target\debug\uninstall_service.exe
```

### 日志

Windows 系统上日志位于：

```
logs/
```

### HTTP API

服务启动后在 `localhost:1145` 提供 HTTP 接口。

#### 查看配置

```bash
curl.exe "http://127.0.0.1:1145/config"
```

#### 设置温度墙 VFP 参考点

```bash
# 设置为 point 44
curl.exe "http://127.0.0.1:1145/set_tem_wall_vfp?point=44"
```

超过温度墙后 GPU 会被设置到指定 VFP 参考点。

#### 设置全局超频频率

```bash
# 设置 OC 为 75
curl.exe "http://127.0.0.1:1145/oc_global?oc=75"
```
