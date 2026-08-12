# 三指拖动（Rust）

这是 `ThreeFingerDragOnWindows` 当前版本的独立 Rust 重写。基础三指拖动和 Swoosh 兼容的进阶状态机都在同一个 Cargo 工作区内运行；旧 C# 程序不参与运行时。

## 功能范围

- Windows 精确式触摸板检测与实时接触点诊断
- 三指拖动，以及左键、右键、中键或不按键模式
- 短暂离开后继续拖动与释放延迟
- 每台触摸板独立的指针移动、速度和加速度
- 开始/结束阈值、单帧移动限制和移动平均
- 托盘打开/退出、单实例运行
- 开机启动、管理员权限运行
- 最近 10,000 行诊断日志与下载目录导出
- 进阶 Rust 手势：窗口半屏/四角/三等分吸附、最大化/最小化/关闭选择、五指自由移动、捏合缩放、鼠标中键 HUD、相邻显示器移动和虚拟桌面溢出新建
- Swoosh 兼容设置：Home、Snapping、Apps 兼容列表/修饰键规则、Appearance 强调色/HUD 主题/尺寸/淡出时间
- 进阶预览覆盖层：标题栏目标判定、实时吸附预览、可取消还原、动画、幽灵触点过滤、演示触点覆盖层和 Windows 强调色
- 进阶设置与基础设置分开保存，进阶模块异常时自动停用且不影响基础拖动

触摸板边缘调节、亮度、媒体控制和其他系统手势均不属于本项目。

## 独立安装与设置

本项目不会读取或修改原项目和其他 Rust 重写版的文件或设置。

- 设置：`%LOCALAPPDATA%\ThreeFingerDragRust\preferences.json`
- 进阶设置：`%LOCALAPPDATA%\ThreeFingerDragRust\advanced-gestures.json`
- 普通启动项名称：`ThreeFingerDragRust`
- 管理员启动任务：`\ThreeFingerDragRust\Run on Startup`
- 日志导出：下载目录中的 `Logs_ThreeFingerDragRust.txt`

第一次运行使用全新默认设置，不导入任何已有偏好；进阶总开关默认为关闭，Swoosh 子设置仍保留源码默认值。

## Swoosh 行为基准

进阶模块按 `bwya77/swoosh` 最新源码提交 `7e3d1b7a8c95d02bc6dc3936f946d9f846a3fafd` 对照实现。源码行为包括标题栏目标判定、两指方向吸附、三等分修饰键、五指移动/居中、捏合与单轴缩放、显示器/虚拟桌面保持手势，以及 Apps 和 Appearance 设置持久化。

## 构建

需要 Windows 10/11、Rust stable、MSVC C++ 构建工具和 WebView2 Runtime。

```powershell
cargo test --workspace
cargo build --release -p three-finger-drag-rust
```

生成的程序位于：

```text
target\release\ThreeFingerDragRust.exe
```

开发时可使用 `--no-elevate` 跳过默认的管理员权限重启：

```powershell
cargo run -p three-finger-drag-rust -- --no-elevate
```

## 项目结构

- `advanced/`：纯 Rust Swoosh 兼容状态机、设置模型、Apps 规则、几何和 FFI 兼容层
- `engine/`：设置、HID/Raw Input、接触点组装、基础/进阶手势运行时、Win32 窗口动作和鼠标输出
- `app/`：Tauri 桌面壳层、托盘、启动项、提权、日志导出
- `ui/`：无前端框架的最小设置界面
- `BEHAVIOR_CONTRACT.md`：与当前 C# 版本对齐的行为契约
- `INDEPENDENT_IMPLEMENTATION.md`：参考边界和未复用内容说明

## 许可

本项目沿用原项目的 MIT 许可。详见 `LICENSE` 和 `NOTICE.md`。
