#!/bin/sh
set -eu

uuid=three-finger-drag@local
target_root=${XDG_DATA_HOME:-"$HOME/.local/share"}/gnome-shell/extensions
target_dir=$target_root/$uuid
legacy_backup_root=$target_root/.three-finger-drag-backups
runtime_files='metadata.json extension.js broker.js exactThreeGuard.js fourFingerDown.js geometry.js hud.js inputOwnership.js interface.xml protocol.js windowBackend.js'
v3_runtime_files='metadata.json extension.js broker.js exactThreeGuard.js geometry.js hud.js inputOwnership.js interface.xml protocol.js windowBackend.js'
legacy_runtime_files='metadata.json extension.js'
rule_path=/etc/udev/rules.d/70-three-finger-drag.rules
config_root=${XDG_CONFIG_HOME:-"$HOME/.config"}
expectation=report
failures=0

usage() {
    printf '%s\n' \
        "用法：$0 [--report|--expect-disabled|--expect-enabled]" \
        '' \
        '--report          只报告扩展状态（默认）' \
        '--expect-disabled 权限阶段或扩展首次手动启用前使用' \
        '--expect-enabled  真实登录会话启用并注销/登录后使用'
}

fail() {
    printf '失败：%s\n' "$*" >&2
    failures=$((failures + 1))
}

find_graphical_session() {
    command -v loginctl >/dev/null 2>&1 || return 1

    candidates=${XDG_SESSION_ID:-}
    listed_sessions=$(loginctl list-sessions --no-legend 2>/dev/null |
        awk -v uid="$(id -u)" '$2 == uid { print $1 }')
    candidates="$candidates $listed_sessions"

    for candidate in $candidates; do
        [ -n "$candidate" ] || continue
        [ "$(loginctl show-session "$candidate" -p User --value 2>/dev/null)" = "$(id -u)" ] ||
            continue
        [ "$(loginctl show-session "$candidate" -p Active --value 2>/dev/null)" = yes ] ||
            continue
        [ "$(loginctl show-session "$candidate" -p Type --value 2>/dev/null)" = wayland ] ||
            continue
        [ "$(loginctl show-session "$candidate" -p Class --value 2>/dev/null)" = user ] ||
            continue
        printf '%s\n' "$candidate"
        return 0
    done

    return 1
}

check_session_is_newer_than() {
    changed_epoch=$1
    changed_label=$2

    if [ "$session_started_epoch" -le "$changed_epoch" ]; then
        fail "当前图形会话启动时间不晚于${changed_label}的安装/修改时间；必须完整注销并重新登录。"
    else
        printf '新登录检查：会话晚于%s\n' "$changed_label"
    fi
}

case ${1:---report} in
    --report) expectation=report ;;
    --expect-disabled) expectation=disabled ;;
    --expect-enabled) expectation=enabled ;;
    --help | -h)
        usage
        exit 0
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
[ "$#" -le 1 ] || {
    usage >&2
    exit 2
}

[ "$(id -u)" -ne 0 ] || fail '必须以真实图形会话的普通用户运行，root 测试不算。'
case ${XDG_CURRENT_DESKTOP:-} in
    *GNOME* | *gnome*) ;;
    *) fail "不是 GNOME 会话：XDG_CURRENT_DESKTOP=${XDG_CURRENT_DESKTOP:-unset}" ;;
esac
[ "${XDG_SESSION_TYPE:-}" = wayland ] ||
    fail "不是 Wayland 会话：XDG_SESSION_TYPE=${XDG_SESSION_TYPE:-unset}"

if command -v gnome-shell >/dev/null 2>&1; then
    shell_version=$(gnome-shell --version | awk '{print $NF}')
    shell_major=${shell_version%%.*}
    printf 'GNOME Shell：%s\n' "$shell_version"
    [ "$shell_major" = 50 ] || fail "只验证了 GNOME Shell 50，当前是 $shell_version。"
else
    fail '缺少 gnome-shell。'
fi
printf '桌面/会话：%s / %s\n' "${XDG_CURRENT_DESKTOP:-unknown}" "${XDG_SESSION_TYPE:-unknown}"

session_id=$(find_graphical_session || true)
if [ -n "$session_id" ]; then
    session_started_text=$(LC_ALL=C loginctl show-session "$session_id" -p Timestamp --value 2>/dev/null || true)
    session_object=$(busctl call org.freedesktop.login1 /org/freedesktop/login1 \
        org.freedesktop.login1.Manager GetSession s "$session_id" 2>/dev/null |
        awk '$1 == "o" { gsub(/"/, "", $2); print $2 }')
    session_started_usec=$(busctl get-property org.freedesktop.login1 "$session_object" \
        org.freedesktop.login1.Session Timestamp 2>/dev/null |
        awk '$1 == "t" { print $2 }')
    case $session_started_usec in
        '' | *[!0-9]*) session_started_epoch=0 ;;
        *) session_started_epoch=$((session_started_usec / 1000000)) ;;
    esac
    if [ "$session_started_epoch" -gt 0 ]; then
        printf '图形会话：%s（%s）\n' "$session_id" "$session_started_text"
    else
        fail "无法通过 logind D-Bus 读取图形会话 $session_id 的启动时间。"
    fi
else
    session_started_epoch=0
    fail '无法通过 logind 找到当前普通用户的活动 Wayland 图形会话。'
fi

if [ -r "$rule_path" ]; then
    printf 'udev 规则：%s\n' "$rule_path"
    grep -Fq 'ENV{ID_INPUT_TOUCHPAD}=="1", TAG+="uaccess"' "$rule_path" ||
        fail 'udev 规则没有精确限定 ID_INPUT_TOUCHPAD=1。'
    grep -Fq 'KERNEL=="uinput", TAG+="uaccess"' "$rule_path" ||
        fail 'udev 规则没有为 uinput 设置活动座席 ACL。'
    if command -v udevadm >/dev/null 2>&1; then
        udevadm verify --no-summary "$rule_path" || fail 'udev 规则语法验证失败。'
    fi
    rule_changed_epoch=$(stat -c '%Y %Z' "$rule_path" 2>/dev/null |
        awk '{ print ($1 > $2) ? $1 : $2 }')
    if [ -n "$rule_changed_epoch" ] && [ "$session_started_epoch" -gt 0 ]; then
        check_session_is_newer_than "$rule_changed_epoch" 'udev 规则'
    else
        fail '无法比较 udev 规则与图形会话的时间。'
    fi
else
    fail "未安装或不可读：$rule_path"
fi

touchpad_count=0
accessible_touchpads=0
for node in /dev/input/event*; do
    [ -e "$node" ] || continue
    if udevadm info --query=property --name="$node" 2>/dev/null |
        grep -Fqx 'ID_INPUT_TOUCHPAD=1'; then
        touchpad_count=$((touchpad_count + 1))
        if [ -r "$node" ]; then
            accessible_touchpads=$((accessible_touchpads + 1))
            if [ -w "$node" ]; then
                printf '触摸板可读：%s（ACL 同时可写，应用仍以 O_RDONLY 打开）\n' "$node"
            else
                printf '触摸板可读：%s\n' "$node"
            fi
        else
            fail "触摸板不可读：$node（请注销并重新登录）"
        fi
    fi
done
[ "$touchpad_count" -gt 0 ] || fail '没有检测到 udev 标记的触摸板。'
[ "$accessible_touchpads" -gt 0 ] || fail '没有可供普通用户读取的触摸板。'

if [ -e /dev/uinput ]; then
    if [ -r /dev/uinput ] && [ -w /dev/uinput ]; then
        printf '%s\n' 'uinput 可读写：/dev/uinput'
    else
        fail '/dev/uinput 不可读写（请注销并重新登录）。'
    fi
else
    fail '/dev/uinput 不存在。'
fi

extension_installed=no
complete_extension=yes
any_extension_file=no
installed_runtime_files=$runtime_files
metadata_version=
for file in $runtime_files; do
    [ -e "$target_dir/$file" ] && any_extension_file=yes
done

if [ -f "$target_dir/metadata.json" ]; then
    if metadata_version=$(python3 - "$target_dir/metadata.json" "$uuid" <<'PY'
import json
import pathlib
import sys

metadata = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
if metadata.get("uuid") != sys.argv[2]:
    raise SystemExit("extension UUID mismatch")
version = metadata.get("version")
if not isinstance(version, int):
    raise SystemExit("extension version must be an integer")
print(version)
PY
    ); then
        case $metadata_version in
            2)
                for broker_file in broker.js exactThreeGuard.js fourFingerDown.js geometry.js hud.js \
                    inputOwnership.js interface.xml protocol.js windowBackend.js; do
                    [ ! -e "$target_dir/$broker_file" ] || {
                        complete_extension=no
                        fail "v2 metadata 与高级 broker 运行文件混用：$broker_file"
                    }
                done
                installed_runtime_files=$legacy_runtime_files
                ;;
            3) installed_runtime_files=$v3_runtime_files ;;
            4 | 5) installed_runtime_files=$runtime_files ;;
            6) installed_runtime_files=$runtime_files ;;
            *)
                complete_extension=no
                fail "不支持的扩展版本：$metadata_version"
                ;;
        esac
    else
        complete_extension=no
        fail "扩展 metadata.json 无法验证：$target_dir"
    fi
fi

[ ! -e "$legacy_backup_root" ] ||
    fail "旧扩展备份目录仍位于 GNOME 扫描路径中：$legacy_backup_root（请迁移到用户数据目录）。"

for file in $installed_runtime_files; do
    [ -f "$target_dir/$file" ] || complete_extension=no
done
if [ "$complete_extension" = yes ]; then
    extension_installed=yes
    printf '用户级扩展：%s（v%s）\n' "$target_dir" "$metadata_version"
    if [ "$metadata_version" = 2 ]; then
        printf '%s\n' '安全分阶段：v2 仅验证精确三指拦截与四指保留，尚未安装高级窗口 broker。'
    fi
    extension_changed_epoch=$(
        for file in $installed_runtime_files; do
            stat -c '%Y %Z' "$target_dir/$file"
        done 2>/dev/null |
            awk '{ for (field = 1; field <= NF; field++) if ($field > newest) newest = $field }
                 END { if (newest != "") print newest }'
    )
    if [ -n "$extension_changed_epoch" ] && [ "$session_started_epoch" -gt 0 ]; then
        check_session_is_newer_than "$extension_changed_epoch" 'GNOME 扩展文件'
    else
        fail '无法比较 GNOME 扩展文件与图形会话的时间。'
    fi
elif [ "$any_extension_file" = yes ]; then
    fail "扩展运行文件不完整：$target_dir（请重新运行安全安装脚本，且保持扩展禁用）"
else
    printf '用户级扩展：尚未安装（权限阶段允许）\n'
fi

extension_enabled=no
extension_active=no
if command -v gnome-extensions >/dev/null 2>&1; then
    if gnome-extensions list --enabled 2>/dev/null | grep -Fqx "$uuid"; then
        extension_enabled=yes
    fi
    if gnome-extensions list --active 2>/dev/null | grep -Fqx "$uuid"; then
        extension_active=yes
    fi
    printf '扩展启用状态：%s\n' "$extension_enabled"
    printf '扩展活动状态：%s\n' "$extension_active"
    if [ "$extension_installed" = yes ]; then
        gnome-extensions info "$uuid" 2>/dev/null ||
            fail 'GNOME Shell 当前会话尚未识别扩展；请注销并重新登录后再检查。'
    elif [ "$extension_enabled" = yes ]; then
        fail '扩展文件不存在，但 GNOME 仍报告为启用；请注销并重新登录后再检查。'
    fi
else
    fail '缺少 gnome-extensions。'
fi

case $expectation in
    disabled)
        [ "$extension_enabled" = no ] || fail '预期扩展未启用，但当前已启用。'
        [ "$extension_active" = no ] || fail '预期扩展未启用，但当前 Shell 仍报告为活动。'
        ;;
    enabled)
        [ "$extension_installed" = yes ] || fail '预期扩展已启用，但扩展文件尚未安装。'
        [ "$extension_enabled" = yes ] || fail '预期扩展已启用，但当前未启用。'
        [ "$extension_active" = yes ] || fail '扩展启用偏好已打开，但当前 Shell 未确认活动。'
        ;;
esac

autostart_entry="$config_root/autostart/three-finger-drag-linux.desktop"
if [ -e "$autostart_entry" ]; then
    [ -f "$autostart_entry" ] || fail '登录启动项不是普通文件。'
    [ "$(stat -c '%u' "$autostart_entry")" = "$(id -u)" ] ||
        fail '登录启动项不属于当前用户。'
    [ "$(stat -c '%a' "$autostart_entry")" = 600 ] ||
        fail '登录启动项权限必须是 600。'
    grep -Fxq 'Type=Application' "$autostart_entry" || fail '登录启动项 Type 无效。'
    grep -Fxq 'OnlyShowIn=GNOME;' "$autostart_entry" || fail '登录启动项未限制为 GNOME。'
    grep -Fxq 'Terminal=false' "$autostart_entry" || fail '登录启动项不应打开终端。'
    grep -Eq '^Exec="/.*" --autostart$' "$autostart_entry" ||
        fail '登录启动项 Exec 未使用带 --autostart 的引号绝对路径。'
    if grep -Eiq 'sudo|pkexec|systemctl|sh -c|bash -c' "$autostart_entry"; then
        fail '登录启动项包含提权、systemd 或 shell 包装。'
    fi
    printf '%s\n' '用户已显式开启 GNOME 登录启动项。'
else
    printf '%s\n' '用户级登录启动保持关闭。'
fi

for forbidden in \
    "$config_root/systemd/user/three-finger-drag-linux.service" \
    /etc/systemd/system/three-finger-drag-linux.service; do
    [ ! -e "$forbidden" ] || fail "安全策略禁止的持久启动项存在：$forbidden"
done

if [ "$failures" -ne 0 ]; then
    printf '验证失败项：%s\n' "$failures" >&2
    exit 1
fi

printf '%s\n' \
    '会话与权限静态检查通过。' \
    '这不能替代实体触摸板测试：请继续验证三指只驱动拖动、四指仍切换工作区/概览。' \
    "Shell 日志检查：journalctl --user -b -o cat | grep -F '$uuid'"
