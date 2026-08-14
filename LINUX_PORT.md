# Linux 移植、Ubuntu 26.04 适配与桌面安全验证

本文档针对 Ubuntu 26.04 LTS、GNOME Shell 50.1 和 Wayland。GNOME 51、其他桌面和 X11 尚未声明兼容，不应绕过脚本的版本检查。静态契约通过不等于实体触摸板验证通过；本文会分开记录“代码已实现”和“当前开发机已实测”。

## 当前开发机状态（不是通用安装前提）

截至本次开发会话：

- `/etc/udev/rules.d/70-three-finger-drag.rules` 已安装，并已在比规则安装更新的真实图形登录会话中，以普通用户验证当前触摸板 event 节点和 `/dev/uinput` ACL；
- 用户已明确报告完成一次全新登录；但当前仓库 v3 代码尚未因本次双指改造重新安装/启用，不得把之前 v2 guard 的状态当成 v3 实测结果；
- 仓库中 v3 已实现双指 evdev/uinput 独占代理和 GNOME 窗口 broker，五指从能力、配置和 UI 强制关闭；离线编译/契约测试通过，实体触摸板动作矩阵仍待分阶段验证；
- 没有创建 GNOME 自启动项、systemd 用户服务或系统级服务。

下一个高风险步骤仍只能是：有备份地安装 v3 扩展但保持高级模式关闭，注销/登录后先验证基础输入；确认稳定后才单独开启双指高级模式。

## 三指与四指为什么冲突

GNOME Shell 50.1 自带的 `SwipeTracker` 以三指为基准，它排除少于三指的事件，却会让三指和四指同时进入工作区/overview 手势路径。Rust 程序又从 evdev 看到同一组物理触点，如果不在 GNOME Shell 侧仲裁，应用的三指拖动就会与 GNOME 系统手势重叠。

v3 扩展 `three-finger-drag@local` 在 `captured-event::touchpad` 阶段跟踪整条物理流，而不是只对单个帧做 `fingers === 3` 判断：

- 独立三指 `BEGIN` 后的流会被 `STOP` 到 `END/CANCEL`；
- 独立四指 `BEGIN` 始终归 GNOME，扩展不会在半途 `STOP`；高级模式可被动观察完整流，仅在确认向下并收到 `END` 后追加全部最小化；
- 已被 `STOP` 的三指流如果中途增加第四指，仍会 `STOP` 到本次流终止。把半条流交回 GNOME 可能使 `SwipeTracker` 只看到 UPDATE/END，这比继续拦截更危险；
- 完全抬手后的下一条独立四指流仍归 GNOME；
- 扩展自身不 grab 物理设备。基础模式仍只读 evdev；只有用户开启高级双指模式时，Rust 代理才临时 grab 并回放非所有流。

扩展元数据只列出 Shell 50，且 `session-modes` 仅包含 `user`；它不应在 GDM 或解锁界面中运行。

## v3 高级 broker 与双指输入代理

实作完成后的 Mutter 50 源码级审计确认：Meta/Wayland 事件过滤器早于 GNOME Shell actor 的 `captured-event::touchpad` 信号运行。对 focused Wayland client，原生 `PINCH/SWIPE/HOLD` 可先通过 pointer-gestures 协议送给客户端，`SCROLL` 也可在更早路径被消费。Shell 扩展后续 `STOP` 无法撤回这些事件。

不能因此直接在 Shell 层拦二指。v3 改为在空触点帧先创建克隆 uinput 触摸板，等待设备节点出现和有界稳定时间，然后才对物理设备执行 `EVIOCGRAB`。基础指针/按键和四指流通过克隆设备送回 libinput；双指候选和三指拖动流不在客户端分发前泄漏。

v3 现在宣告 `twoFinger=true`、`fiveFinger=false`、`threeFinger=true`、`fourFinger=false`。`twoFinger` 是 Rust 代理与 Shell broker 组合能力，不是对 `captured-event` 时序的错误声明。代理创建、grab、uinput 写入、D-Bus 或终态队列任一失败时均 fail closed 窗口事务并 fail open 物理触摸板。

`fourFinger=false` 仍表示 broker 不抢占四指输入所有权。新增的 `minimizeAll=true` 是被动 Shell 动作：完整四指流依然向 GNOME 传播，只在物理向下距离达到阈值、纵向明显占优且手势正常结束时，最小化当前工作区的全部普通窗口。左、右、上与取消流不执行任何附加动作。

D-Bus 界面是：

```text
bus:       io.github.xuzenghui942.ThreeFingerDrag.Gnome
object:    /io/github/xuzenghui942/ThreeFingerDrag/Gnome
interface: io.github.xuzenghui942.ThreeFingerDrag.Gnome1
methods:   GetCapabilities, GetModifiers, Configure,
           Begin, Update, Commit, Cancel
```

`Begin` 不接受 PID、窗口 ID 或目标 ID。扩展根据当前指针位置在合成器内选择并锁定可管理窗口。只有正常、可见、非全屏、非 attached-dialog/override-redirect 窗口，且指针在其保守的顶部边带内，才能成为目标。应用标识的 exclude/require-modifier 兼容规则也在选择时执行。

### 双指所有权与窗口动作

- 第二个触点出现时，代理先向克隆设备发送所有 slot/按键释放，然后才进入双指候选；
- broker 只能在指针下的可管理窗口顶部安全带接受 `Begin`，不接受 PID/窗口 ID/任意目标；
- `BeginRejected` 时代理按当前 slot/tracking ID/坐标和按键重建输入，后续帧保持原生到本次全抬手；
- 三指候选由基础拖动所有；四指从当前完整触点快照重建到克隆设备，之后保持原生到抬手；
- 连续双指帧最多以约 60 Hz 进入手势引擎，但触点数变化/终态帧立即处理；D-Bus 队列为 Commit/Cancel 保留终态槽位。

当前声明可用的双指窗口动作包括：

- 半屏、四分之一、最大化和向下窗口动作；
- 最大化和最小化；
- 移动到已存在的相邻工作区；
- 移动到已存在的相邻显示器；
- 水平/垂直单轴调整大小；
- 捏合提交、取消时还原原窗口/工作区/显示器状态；
- 向下最小化/关闭/手势内二选一，包括回拉取消并恢复之前停留的吸附方向；
- 按住后选择工作区，或在开启“应用切换”时选择要接管目标位置的窗口；
- 关闭实时预览时用 Shell HUD 显示目标区，开启时移动真实窗口并在取消时还原；吸附动画可单独开关；
- 识别期间不模拟鼠标左键，也不让指针跟着双指轨迹移动；只有开启“鼠标跟随窗口”时，最终吸附才按窗口移动前的指针相对偏移重定位。

另外包括只调用 `Meta.Window.delete(time)` 的优雅关闭，以及右侧边界的动态工作区创建。五指自由移动/自由缩放和五指居中不在 Linux 产品范围内，能力始终为 `false`。

## 已准备的开发环境

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

用户级工具链：

- 当前验证 Rust/Cargo 1.97.1，项目 `rust-toolchain.toml` 跟随 `stable`；
- rustfmt 1.9.0-stable、Clippy 0.1.97；
- Tauri CLI 2.x，当前验证为 2.11.4；
- Zig 0.16.0 只作显式 compiler fallback，不写入系统编译器配置。

检查环境：

```bash
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
./scripts/bootstrap-dev.sh --check
```

如 Ubuntu 依赖尚未齐全，可明确要求脚本只安装官方软件包：

```bash
./scripts/bootstrap-dev.sh --install-system
```

该脚本不接触 GNOME 扩展、udev、GDM、PAM、systemd 或自启动。如已安装 `build-essential` 仍没有系统 `cc`，只对当前构建命令显式使用 Zig fallback：

```bash
CC="$HOME/.local/bin/zig-cc" cargo build --workspace
```

## 安全原则

udev 活动座席 ACL 和 GNOME 扩展都是输入/桌面高风险改动。必须一次只做一项，手工测试后注销并全新登录，再决定是否继续。任何一项失败时先恢复上一个已知正常状态，不叠加修复。

仓库不会自动创建或启用：

- 任何未经用户开关明确授权的 `~/.config/autostart/*.desktop`；
- `~/.config/systemd/user/*.service`；
- 系统级 systemd 服务；
- GDM、PAM、Wayland/Xorg 或 GNOME session 配置。

主程序必须以普通用户运行，且会拒绝 root。登录自启动默认关闭；用户在界面明确开启时，只以 0600 原子创建 `~/.config/autostart/three-finger-drag-linux.desktop`，`Exec` 为引号绝对路径加 `--autostart`，限制 `OnlyShowIn=GNOME;`，不包含 shell、sudo/pkexec 或 systemd。关闭开关会删除该文件。

## 阶段 0：离线构建与契约测试

```bash
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
gnome-extension/three-finger-drag@local/tests/static-contract.sh
cargo build --workspace
```

GNOME 扩展的静态契约会验证 JSON/XML、D-Bus 签名、协议/几何 fixture、输入流所有权、GNOME 50 GI API 和 GJS 语法。它不加载或启用扩展，不能代替实体触摸板测试。

## 阶段 1：单独安装并验证 udev 权限

唯一会修改的系统文件是：

```text
/etc/udev/rules.d/70-three-finger-drag.rules
```

脚本会在写入前验证模板，把旧规则完整备份到 `/var/backups/three-finger-drag/`，并打印可用的回滚命令。规则只对 `ID_INPUT_TOUCHPAD=1` 的 event 节点和 `/dev/uinput` 添加 logind `uaccess`，不设置 `MODE="0666"`，不把用户加入可读全部输入设备的宽泛组。

只读预检：

```bash
./scripts/install-input-permissions.sh --check
sed -n '1,120p' ./scripts/70-three-finger-drag.rules
udevadm verify --no-summary ./scripts/70-three-finger-drag.rules
```

在保存脚本打印的回滚命令后明确安装：

```bash
pkexec ./scripts/install-input-permissions.sh --install
```

安装后脚本只 reload udev 规则，并精确 trigger 触摸板和 uinput；它不启用服务。现在停止，不安装扩展。注销并重新登录，在真实普通用户 Wayland 会话运行：

```bash
./scripts/verify-linux-session.sh --expect-disabled
```

验证脚本会比较 logind 图形会话启动时间与规则安装时间。旧会话中 reload/trigger 后拿到 ACL 不算通过。随后手动启动应用，检查触点诊断、拖动、退出后释放按键和热插拔。此时如果尚未启用 guard，GNOME 三指动画可能与应用重叠；这是已知中间状态，不应用其他手势工具叠加修复。

回滚（使用安装时实际打印的备份路径）：

```bash
pkexec ./scripts/install-input-permissions.sh --rollback '/var/backups/three-finger-drag/时间戳/70-three-finger-drag.rules'
```

首次安装没有旧规则时：

```bash
pkexec ./scripts/install-input-permissions.sh --uninstall
```

回滚/卸载可从 TTY 执行，随后必须再注销和重新登录。

## 阶段 2：基础 guard 实机验证

这一阶段只验证三指/四指所有权，高级模式必须保持关闭。

### 当前开发机的 v2 续测流程

当前 v2 已复制、仍禁用，因此不得重新执行安装脚本。首先注销并全新登录，然后：

```bash
./scripts/verify-linux-session.sh --expect-disabled
gnome-extensions info three-finger-drag@local
journalctl --user -b -o cat | grep -F three-finger-drag@local
```

`gnome-extensions info` 必须成功，日志不能有该扩展的解析、导入或版本错误。再由用户手动启用：

```bash
gnome-extensions enable three-finger-drag@local
```

完成本节后面的“基础所有权矩阵”，保持应用手动启动。通过后再注销/全新登录，运行 `verify-linux-session.sh --expect-enabled` 并重复矩阵。任何异常都立即禁用/回滚 v2，不升级 v3。

### 新机器或没有 v2 的基线流程

不要把上述“v2 已安装”当作通用事实。当前仓库发布的是 v3；它在 Ubuntu 26.04 上只对外提供 fail-closed 的精确三指 guard。新机器可按下一阶段安装 v3，但首次启用后仍必须先进行基础矩阵和注销/全新登录；二/五指高级模式仍保持关闭。

### 基础所有权矩阵

| 测试 | 预期结果 |
| --- | --- |
| 一指移动/点击 | 与启用前一致 |
| 普通客户区二指滚动 | 与启用前一致 |
| 应用未运行时独立三指滑动 | GNOME 不切换工作区、不进 overview |
| 应用运行时三指拖动 | 只有应用拖动，GNOME 不同时触发 |
| 三指流中途加第四指 | 该流仍被停止到完全抬手，GNOME 不半途接手 |
| 完全抬手后新的四指横向滑动 | GNOME 工作区切换仍可用 |
| 高级模式关闭时新的四指纵向滑动 | GNOME overview 仍可用 |
| 高级模式开启且四指向下 | 当前工作区的全部普通窗口最小化 |
| 高级模式开启且四指左/右/上 | 仍由 GNOME 执行默认行为，不最小化窗口 |
| 应用正常退出/异常停止 | 虚拟按键释放，无卡键 |
| 锁屏/解锁 | 扩展不在解锁 session mode 运行，返回桌面后状态正常 |

## 阶段 3：独立升级/安装完整 v5 broker

对当前开发机，只能在 v2 基础矩阵与其后注销/全新登录复验都通过后进入本阶段。先禁用 v2：

```bash
gnome-extensions disable three-finger-drag@local
```

安装脚本会原子替换整个用户扩展目录，并在覆盖前将旧 v2 目录完整备份到：

```text
${XDG_DATA_HOME:-$HOME/.local/share}/three-finger-drag-linux/gnome-extension-backups/
```

v5 安装目标是下列十一个运行文件，不再是只有 `metadata.json` 和 `extension.js`：

```text
~/.local/share/gnome-shell/extensions/three-finger-drag@local/metadata.json
~/.local/share/gnome-shell/extensions/three-finger-drag@local/extension.js
~/.local/share/gnome-shell/extensions/three-finger-drag@local/broker.js
~/.local/share/gnome-shell/extensions/three-finger-drag@local/exactThreeGuard.js
~/.local/share/gnome-shell/extensions/three-finger-drag@local/fourFingerDown.js
~/.local/share/gnome-shell/extensions/three-finger-drag@local/geometry.js
~/.local/share/gnome-shell/extensions/three-finger-drag@local/hud.js
~/.local/share/gnome-shell/extensions/three-finger-drag@local/inputOwnership.js
~/.local/share/gnome-shell/extensions/three-finger-drag@local/interface.xml
~/.local/share/gnome-shell/extensions/three-finger-drag@local/protocol.js
~/.local/share/gnome-shell/extensions/three-finger-drag@local/windowBackend.js
```

如使用非默认 `XDG_DATA_HOME`，上述 `~/.local/share` 前缀会随之改变。

先只读检查：

```bash
./scripts/install-gesture-guard.sh --check
```

`--check` 会检查 Ubuntu/GNOME/Wayland 版本、十一个运行文件和静态契约，不写文件。确认脚本打印的准确变更、备份位置和回滚命令后，以普通用户明确安装：

```bash
./scripts/install-gesture-guard.sh --install
```

脚本不会调用 `gnome-extensions enable`，不会创建自启动或服务。安装后扩展必须仍处于禁用状态。现在注销并全新登录，先做加载前检查：

```bash
./scripts/verify-linux-session.sh --expect-disabled
gnome-extensions info three-finger-drag@local
journalctl --user -b -o cat | grep -F three-finger-drag@local
```

当前 logind 会话必须晚于十个扩展运行文件。信息和日志无导入/API 错误后，用户手动启用：

```bash
gnome-extensions enable three-finger-drag@local
```

保持应用的高级主开关关闭，重复“基础所有权矩阵”，再注销/全新登录并复测。这是 v3 guard 的基线；Ubuntu 26.04 上不打开二指或五指窗口操作。

升级失败时使用安装脚本实际打印的备份目录：

```bash
gnome-extensions disable three-finger-drag@local
./scripts/install-gesture-guard.sh --rollback '安装脚本打印的备份目录'
```

首次安装无旧版本时：

```bash
gnome-extensions disable three-finger-drag@local
./scripts/install-gesture-guard.sh --uninstall
```

回滚/卸载都先把当前版本移到可恢复备份目录，不会启用恢复后的扩展。完成后注销并重新登录。

## 阶段 4：Ubuntu 26.04 双指高级实机验证

v3 在“高级关闭”状态通过基础矩阵和注销/新登录后，才能单独打开双指高级主开关：

1. 开启前确认普通单指指针、物理按键、客户区双指滚动、三指拖动和四指 GNOME 手势都正常。
2. 打开高级主开关，等待 UI 报告 broker 运行中；检查 `/dev/input/by-id`/日志只有每个物理触摸板对应一个 `Three Finger Drag proxied touchpad`，不得递归复制。
3. 指针在客户区时重复滚动、浏览器缩放和 hold；不得移动窗口，也不得丢失普通滚动。
4. 指针在顶部安全带时，先关闭“鼠标跟随窗口”和“实时预览”：滑动中应只显示目标 HUD，指针保持原地，窗口在松手后提交。再开启实时预览，确认真实窗口随选区切换且 Escape/取消可还原。最后单独开启鼠标跟随，确认最终吸附后指针保持在窗口内的原相对像素位置，全程不得模拟左键或跟随手指轨迹。
5. 逐项验证所有双指动作：半屏/四分之一、最大化与还原、最小化、优雅关闭/下滑选择与回拉、按住工作区/应用切换、现有/动态工作区、显示器移动、水平/垂直调整大小和捏合。
6. 在每类动作的 Begin/Update 阶段按 Escape、移除目标窗口、拔插外接触摸板，确认窗口几何/工作区/显示器还原且输入不锁死。
7. 关闭高级主开关，确认物理 grab 在全抬手后解除；注销并全新登录后重复基础矩阵。

root、`pkexec`、容器、嵌套 Shell 或自动合成输入不能代替这些实体触摸板验证。任一步失败时先关闭高级主开关并退出应用，不叠加修复。

## 图形会话失效时从 TTY 恢复

在进行扩展测试前确认 `Ctrl`+`Alt`+`F3` 可切换到文字终端，并保存安装脚本打印的备份路径。如图形会话不可用，以同一普通用户在 TTY 登录，先禁用所有用户扩展并把本扩展移出加载路径：

```bash
dbus-run-session -- gsettings set org.gnome.shell disable-user-extensions true
./scripts/install-gesture-guard.sh --uninstall
```

如已知需要恢复 v2 或其他已验证版本，也可在扩展禁用后使用安装时打印的 `--rollback BACKUP_DIRECTORY`。然后退出 TTY 并重启或返回新的图形登录。确认桌面恢复后，才可恢复其他用户扩展：

```bash
dbus-run-session -- gsettings set org.gnome.shell disable-user-extensions false
```

如故障发生在输入权限修改后，先执行 udev 安装脚本打印的 `--rollback` 命令；没有旧规则则执行 `--uninstall`。不修改 GDM、PAM、Wayland/Xorg 配置，不叠加新手势工具。

## 当前边界

- GNOME 扩展仅声明 Ubuntu 26.04 / GNOME Shell 50 / Wayland 用户会话组合；
- 三指/四指输入所有权、四指向下全部最小化与双指代理必须用实体触摸板手工证明；五指高级能力在 Linux 保持明确关闭；
- evdev/uinput ACL 必须在注销/全新登录后以普通用户证明，root 可访问不算；
- close 仅使用合成器优雅关闭，绝不 kill 进程；动态工作区只在用户开启对应选项时从右侧边界创建；
- 本仓库不会自动创建自启动；只有用户界面开关明确开启才创建单一 GNOME `.desktop` 项。
