# NVOC-srv

## 配套产品——使用所有配套产品以达到最好体验

[NVOC-AUTOOPTIMIZER](https://github.com/Skyworks-Neo/NVOC-AutoOptimizer)：核心模块。

[NVOC-STRESSOR](https://github.com/Skyworks-Neo/NVOC-CLI-Stressor)：压力测试模块，用于自动超频扫描部分。没有该模块仍可以使用自动扫描之外的所有功能。（NVOC-AutoOptimizer开放任何你的自定义压力测试模块接入，只需满足return
code定义即可。）

[NVOC-GUI](https://github.com/Skyworks-Neo/NVOC-GUI)：跨平台超频图形界面，直接对标MSI Afterburner。 （为了避免GPU超炸带走图形界面，使用CPU渲染，在低端机器如遇到性能问题，建议使用NVOC-TUI）；

[NVOC-TUI](https://github.com/Skyworks-Neo/NVOC-TUI)：跨平台超频命令行界面，用于没有图形界面的机器，兼容性好，性能要求低；

[NVOC-SRV](https://github.com/Skyworks-Neo/NVOC-SRV)：client-server架构控制模块，用于机房、服务器、工作站等场景的 Web 管理、~~远程超频~~（TODO）

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
