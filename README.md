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
logs/
```

## 3 Parameter update

A web service is on the 1145 port of localhost.

Now the example is a temperature wall of 64C. Over 64C will be set to vfp ref point 48.


To change the ref point to 44
```
curl.exe "http://127.0.0.1:1145/set_tem_wall_vfp?point=44"
```

To check the ref point
```
curl.exe "http://127.0.0.1:1145/config"
```

To set a global OC frequency
```
curl.exe "http://127.0.0.1:1145/oc_global?oc=75"
```