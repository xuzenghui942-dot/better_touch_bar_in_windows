# Better Touch Bar Linux（Rust）

这是面向 Ubuntu GNOME Wayland 的本地触摸板工具。基础三指拖动以只读、非阻塞方式观察 Linux evdev，再通过 uinput 注入虚拟鼠标。只有用户显式启用高级双指窗口手势时，后端才会临时独占物理触摸板，并把非高级输入通过克隆的 uinput 触摸板回放给 libinput。

当前针对 Ubuntu 26.04 LTS、GNOME Shell 50.1 和 Wayland 开发。原 Windows 项目只作为历史行为参考，不参与 Linux 运行时。

## 当前实现

基础三指拖动包括：

- 模拟左键、右键、中键或只移动指针；
- 短暂离开后续拖、可调释放延迟、开始/结束阈值、速度、加速和平滑；
- 每台触摸板独立设置，支持热插拔重扫描、`SYN_DROPPED` 恢复和安全释放虚拟按键；
- 实时触点预览、单实例、托盘、最近 10,000 行诊断日志与导出。

GNOME Shell 50 默认的工作区/overview 手势会同时接收三指和四指滑动。本仓库的 GNOME 50 用户会话扩展在 Clutter 捕获阶段取得精确的输入所有权：

- 独立的三指流由扩展 `STOP`，避免与应用的三指拖动重叠；
- 独立从四指 `BEGIN` 开始的流始终完整交给 GNOME；高级模式只被动观察完整流，在确认四指向下结束后最小化当前工作区的全部普通窗口；
- 如果一条已被 `STOP` 的三指流中途增加第四指，扩展会继续停止该物理流直到 `END/CANCEL`，不把半条流临时交给 GNOME；
- 扩展不拦截 Rust 后端的只读 evdev 触点。

## Ubuntu 26.04 高级双指窗口手势

高级模式默认关闭。GNOME Shell 的事件信号晚于 Wayland 客户端分发，所以本实现不用 Shell `STOP` 伪装成二指互斥，而是在 evdev 层临时使用 `EVIOCGRAB`：

- 先创建与物理设备轴、按键和属性匹配的虚拟触摸板，等待 libinput 识别后才 grab；
- 双指候选流不会先送给浏览器/GTK。GNOME broker 先在标题栏内锁定窗口，Rust 与 Windows 版使用同一套方向、按住、捏合和轴缩放状态机；识别期间不会模拟左键，也不会让指针跟着手指移动；
- 关闭“实时预览”时显示目标区域 HUD，窗口只在松手后移动；开启时真实窗口会在选区间预览，取消则还原；
- “鼠标跟随窗口”是独立可选项：关闭时指针原地不动；开启时只在最终吸附后按窗口移动前的相对像素偏移重定位，并不跟随双指轨迹；
- 指针不在可管理标题栏时，Rust 在 `BeginRejected` 后重建当前触点，普通双指滚动继续交给 libinput；
- 三指由拖动后端所有，四指会重建到虚拟触摸板并保持由 GNOME 处理；只有启用的“四指向下全部最小化”会在手势完整结束后附加执行；
- 任何 uinput 写入、D-Bus 或队列终态失败都会取消窗口事务，解除物理设备独占或抑制到本次抬手。

`twoFinger=true`，二指窗口动作 capability 按实现声明为可用；`minimizeAll=true` 表示可用被动四指向下动作；`fiveFinger=false`，Linux 配置、运行时和 UI 都会强制关闭五指功能。

## 开发环境

Ubuntu 官方构建依赖：

```text
libwebkit2gtk-4.1-dev
build-essential
libxdo-dev
libssl-dev
libayatana-appindicator3-dev
librsvg2-dev
patchelf
shellcheck
```

当前已验证 Rust/Cargo 1.97.1、rustfmt、Clippy、Tauri CLI 2.11.4 和 Zig 0.16.0。`rust-toolchain.toml` 跟随 Rust `stable`；Zig 只在系统 `cc` 不可用时作为显式 fallback。

```bash
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
./scripts/bootstrap-dev.sh --check
# 如确实缺少上述 Ubuntu 官方包：
./scripts/bootstrap-dev.sh --install-system
```

`bootstrap-dev.sh` 不安装或启用 GNOME 扩展、udev 规则、systemd 服务或桌面自启动。

## 构建与静态测试

```bash
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
gnome-extension/three-finger-drag@local/tests/static-contract.sh
cargo build --release -p three-finger-drag-rust
```

发行二进制位于 `target/release/three-finger-drag-linux`。生成 deb/AppImage：

```bash
cd app
cargo tauri build
```

## 安全安装顺序

udev 权限与 GNOME 扩展都是高风险桌面改动，必须每次只改一项，在真实、非特权图形会话中手工测试，并通过注销/全新登录后再进入下一阶段。安装脚本会显示完整备份和回滚命令，但不会启用扩展。

通用顺序是：

1. 完成构建和离线测试。
2. 单独安装 udev `uaccess` 规则，注销/登录后验证 ACL，再以普通用户手动运行基础应用。
3. 单独安装 GNOME 扩展源码但保持禁用，注销/登录后先确认 Shell 能正常识别。
4. 手动启用扩展，保持高级模式关闭，先验证基础三指/四指所有权，并再经历一次注销/登录。
5. 保持高级模式关闭先验证普通单指/双指、三指和四指；然后单独开启高级模式，逐项测试双指窗口动作、客户区滚动回放和四指向下全部最小化。

开发机上之前安装的基础 v2 guard 不能当作当前 v3 双指代理的实测结果。v3 必须作为一次独立、有备份的升级安装，保持高级关闭经历注销/登录与基础矩阵后，再单独开启双指测试。完整状态、十个运行文件、测试矩阵和 TTY 恢复命令见 [LINUX_PORT.md](LINUX_PORT.md)。

## 手动运行

完成权限验证后，在真实、非特权 GNOME Wayland 会话中运行：

```bash
cargo run -p three-finger-drag-rust
# 或
./target/release/three-finger-drag-linux
```

关闭设置窗口只会隐藏到托盘；请从托盘或界面显式退出，让输入后端安全解除触摸板独占并释放虚拟按键。“登录时启动”默认关闭；用户显式开启时只创建 `~/.config/autostart/three-finger-drag-linux.desktop`，不创建 systemd 服务，关闭开关即删除该文件。

## 数据与项目结构

- 基础设置：`${XDG_DATA_HOME:-~/.local/share}/three-finger-drag-linux/preferences.json`；
- 高级设置：`${XDG_DATA_HOME:-~/.local/share}/three-finger-drag-linux/advanced-window-gestures.json`；
- 日志导出：用户下载目录中的 `three-finger-drag-linux.log`；
- `engine/src/linux/`：evdev/uinput 和 GNOME broker 传输、所有权与设备生命周期；
- `app/` 与 `ui/`：Tauri 桌面壳、状态、能力导向的 Linux 高级设置和诊断；
- `gnome-extension/three-finger-drag@local/`：GNOME 50 精确三指 guard 与高级窗口 broker；
- `scripts/`：开发环境检查、udev/扩展安全安装、回滚和登录会话验证。

## 许可

MIT。详见 `LICENSE` 和 `NOTICE.md`。
