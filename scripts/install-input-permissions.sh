#!/bin/sh
set -eu

rule_path=/etc/udev/rules.d/70-three-finger-drag.rules
backup_root=/var/backups/three-finger-drag
script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
script_path=$script_dir/$(basename -- "$0")
source_rule=$script_dir/70-three-finger-drag.rules

usage() {
    printf '%s\n' \
        '用法：' \
        "  $0 --check" \
        "  pkexec $0 --install" \
        "  pkexec $0 --rollback BACKUP_FILE" \
        "  pkexec $0 --uninstall" \
        '' \
        '--check 只检查；其他模式均要求明确参数和 root 权限。'
}

die() {
    printf '错误：%s\n' "$*" >&2
    exit 1
}

require_root() {
    [ "$(id -u)" -eq 0 ] || die "请使用 pkexec '$script_path' $*"
}

check_recovery_tools() {
    command -v udevadm >/dev/null 2>&1 || die '缺少 udevadm，无法 reload 规则。'
}

check_compatibility() {
    [ -r /etc/os-release ] || die '无法读取 /etc/os-release。'
    # shellcheck disable=SC1091
    . /etc/os-release
    [ "${ID:-}" = ubuntu ] || die "仅验证了 Ubuntu 26.04；检测到 ${PRETTY_NAME:-unknown}。"
    [ "${VERSION_ID:-}" = 26.04 ] || die "仅验证了 Ubuntu 26.04；检测到 ${PRETTY_NAME:-unknown}。"
    command -v udevadm >/dev/null 2>&1 || die '缺少 udevadm。'
    udev_version=$(udevadm --version)
    [ "$udev_version" -ge 259 ] 2>/dev/null || die "需要 systemd/udev 259 或更高；检测到 $udev_version。"
    [ -f /usr/lib/udev/rules.d/73-seat-late.rules ] ||
        die '缺少 73-seat-late.rules，无法确认 TAG+=uaccess 会授予活动座席 ACL。'
    grep -Fq 'RUN{builtin}+="uaccess"' /usr/lib/udev/rules.d/73-seat-late.rules ||
        die '系统 uaccess 规则与已验证的 Ubuntu 26.04 不一致。'

    touchpad_count=0
    for node in /dev/input/event*; do
        [ -e "$node" ] || continue
        if udevadm info --query=property --name="$node" 2>/dev/null |
            grep -Fqx 'ID_INPUT_TOUCHPAD=1'; then
            touchpad_count=$((touchpad_count + 1))
            printf '检测到触摸板：%s\n' "$node"
        fi
    done
    [ "$touchpad_count" -gt 0 ] || die 'udev 未检测到 ID_INPUT_TOUCHPAD=1 的触摸板。'

    printf '%s\n' \
        "兼容性检查通过：${PRETTY_NAME:-Ubuntu 26.04} / udev $udev_version" \
        "将精确修改：$rule_path" \
        '安装后只执行 udev 规则 reload，并仅 trigger 已标记的触摸板与 uinput。' \
        '不会启用、创建或重启 systemd 服务，也不会创建桌面自启动。'
}

write_candidate_rule() {
    candidate_rule=$1
    [ -f "$source_rule" ] || die "缺少 udev 规则模板：$source_rule"
    udevadm verify --no-summary "$source_rule"
    install -o root -g root -m 0644 "$source_rule" "$candidate_rule"
    udevadm verify --no-summary "$candidate_rule"
}

reload_and_trigger() {
    udevadm control --reload-rules &&
        udevadm trigger --action=change --subsystem-match=input \
            --property-match=ID_INPUT_TOUCHPAD=1 --settle &&
        udevadm trigger --action=change --subsystem-match=misc \
            --sysname-match=uinput --settle
}

make_stamp() {
    date +%Y%m%d-%H%M%S
}

install_rule() {
    require_root --install
    check_compatibility
    [ ! -L "$rule_path" ] || die "拒绝替换符号链接：$rule_path"

    stamp=$(make_stamp)
    backup_path=$backup_root/$stamp-$$/$(basename -- "$rule_path")
    candidate_rule=$(mktemp /etc/udev/rules.d/.70-three-finger-drag.XXXXXX.rules)
    trap 'test ! -e "$candidate_rule" || rm -f -- "$candidate_rule"' EXIT HUP INT TERM
    write_candidate_rule "$candidate_rule"

    printf '%s\n' \
        '' \
        "将精确修改：$rule_path" \
        '将 reload udev 规则，并只 trigger ID_INPUT_TOUCHPAD=1 和 uinput 设备。' \
        '不会修改 GDM、PAM、GNOME 会话、systemd 服务或桌面自启动。' \
        '安全说明：触摸板 ACL 允许读取触点；uinput ACL 允许活动用户合成输入。'
    if [ -e "$rule_path" ]; then
        printf '%s\n' \
            "现有规则将先备份到：$backup_path" \
            '应用前回滚命令：' \
            "  pkexec '$script_path' --rollback '$backup_path'"
        mkdir -p "$(dirname -- "$backup_path")"
        cp -a -- "$rule_path" "$backup_path"
        printf '已备份现有规则：%s\n' "$backup_path"
        had_previous=yes
    else
        printf '%s\n' \
            '当前没有旧规则；应用前回滚命令：' \
            "  pkexec '$script_path' --uninstall"
        had_previous=no
    fi

    mv -- "$candidate_rule" "$rule_path"
    trap - EXIT HUP INT TERM

    if ! reload_and_trigger; then
        failed_path=$backup_root/failed-install-$stamp-$$/$(basename -- "$rule_path")
        mkdir -p "$(dirname -- "$failed_path")"
        mv -- "$rule_path" "$failed_path"
        if [ "$had_previous" = yes ]; then
            cp -a -- "$backup_path" "$rule_path"
        fi
        reload_and_trigger || true
        die "udev reload/trigger 失败；已恢复应用前状态，新规则保存在 $failed_path。"
    fi

    printf '%s\n' \
        '权限规则已安装并重新加载。' \
        '请注销并重新登录；随后以普通用户运行 scripts/verify-linux-session.sh。' \
        '不要以 root 运行主程序。'
}

validate_backup_path() {
    candidate=$1
    [ -f "$candidate" ] || die "备份文件不存在：$candidate"
    [ ! -L "$candidate" ] || die "拒绝恢复符号链接备份：$candidate"
    [ "$(basename -- "$candidate")" = "$(basename -- "$rule_path")" ] ||
        die "备份文件名必须为 $(basename -- "$rule_path")。"
    backup_path=$(CDPATH='' cd -- "$(dirname -- "$candidate")" && pwd -P)/$(basename -- "$candidate")
    backup_root_real=$(CDPATH='' cd -- "$backup_root" 2>/dev/null && pwd -P) ||
        die "备份根目录不存在：$backup_root"
    case $backup_path in
        "$backup_root_real"/*) ;;
        *) die "拒绝使用备份根目录之外的路径：$backup_path" ;;
    esac
    udevadm verify --no-summary "$backup_path"
}

rollback_rule() {
    require_root --rollback
    [ "$#" -eq 1 ] || die '--rollback 需要一个备份文件。'
    # 恢复必须能在 TTY 和触摸板未被识别时执行，因此不重复安装时的
    # GNOME/硬件兼容性门禁，只要求恢复所必需的 udevadm。
    check_recovery_tools
    validate_backup_path "$1"
    [ ! -L "$rule_path" ] || die "拒绝替换符号链接：$rule_path"

    stamp=$(make_stamp)
    displaced_path=$backup_root/rollback-displaced-$stamp-$$/$(basename -- "$rule_path")
    printf '%s\n' \
        "将恢复：$backup_path" \
        "到：$rule_path" \
        "当前规则（如存在）将先移到：$displaced_path" \
        '随后只 reload/trigger udev；不会修改服务或自启动。'

    had_current=no
    if [ -e "$rule_path" ]; then
        mkdir -p "$(dirname -- "$displaced_path")"
        mv -- "$rule_path" "$displaced_path"
        had_current=yes
    fi
    cp -a -- "$backup_path" "$rule_path"

    if ! reload_and_trigger; then
        failed_path=$backup_root/failed-rollback-$stamp-$$/$(basename -- "$rule_path")
        mkdir -p "$(dirname -- "$failed_path")"
        mv -- "$rule_path" "$failed_path"
        if [ "$had_current" = yes ]; then
            mv -- "$displaced_path" "$rule_path"
        fi
        reload_and_trigger || true
        die "恢复后的规则未能 reload/trigger；已恢复回滚前状态，失败副本位于 $failed_path。"
    fi

    printf '%s\n' '旧规则已恢复。请注销并重新登录后验证输入与图形会话。'
}

uninstall_rule() {
    require_root --uninstall
    # 与 --rollback 一样，保持 TTY 恢复路径可用。
    check_recovery_tools
    [ ! -L "$rule_path" ] || die "拒绝移动符号链接：$rule_path"
    if [ ! -e "$rule_path" ]; then
        printf '%s\n' '规则不存在，无需卸载。'
        return
    fi

    stamp=$(make_stamp)
    removed_path=$backup_root/removed-$stamp-$$/$(basename -- "$rule_path")
    printf '%s\n' \
        "将把规则移到可恢复位置：$removed_path" \
        '随后只 reload/trigger udev；不会修改服务或自启动。'
    mkdir -p "$(dirname -- "$removed_path")"
    mv -- "$rule_path" "$removed_path"

    if ! reload_and_trigger; then
        mv -- "$removed_path" "$rule_path"
        reload_and_trigger || true
        die '卸载后的 udev reload/trigger 失败；已恢复原规则。'
    fi
    printf '%s\n' '规则已移出 /etc；请注销并重新登录。'
}

action=${1:---check}
case $action in
    --check)
        [ "$#" -eq 1 ] || die '--check 不接受其他参数。'
        check_compatibility
        printf '%s\n' '只读检查完成；没有修改规则或设备。'
        ;;
    --install)
        [ "$#" -eq 1 ] || die '--install 不接受其他参数。'
        install_rule
        ;;
    --rollback)
        shift
        rollback_rule "$@"
        ;;
    --uninstall)
        [ "$#" -eq 1 ] || die '--uninstall 不接受其他参数。'
        uninstall_rule
        ;;
    --help | -h)
        usage
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
