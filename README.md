# NVOC-srv

Windows Service for Nvidia GPU Optimizer

## 0 Compile

clone this repo and nvoc-auto-optimizer

├── NVOC-AutoOptimizer
├── nvoc-srv

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
.\notify\debug\uninstall_service.exe
```

## 2 Check log

Usually on a Windows System Computer the path is

```
C:\Windows\System32\canbedel-nvoc_service-output.log
```