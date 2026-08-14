#!/bin/sh
set -eu

uuid=three-finger-drag@local
script_path=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)/$(basename -- "$0")
source_dir=$(CDPATH='' cd -- "$(dirname -- "$0")/../gnome-extension/$uuid" && pwd)
target_root=${XDG_DATA_HOME:-"$HOME/.local/share"}/gnome-shell/extensions
target_dir=$target_root/$uuid
backup_root=${XDG_DATA_HOME:-"$HOME/.local/share"}/three-finger-drag-linux/gnome-extension-backups
runtime_files='metadata.json extension.js broker.js exactThreeGuard.js fourFingerDown.js geometry.js hud.js inputOwnership.js interface.xml protocol.js windowBackend.js'

usage() {
    printf '%s\n' \
        '用法：' \
        "  $0 --check" \
        "  $0 --install" \
        "  $0 --rollback BACKUP_DIRECTORY" \
        "  $0 --uninstall" \
        '' \
        '--check 仅做兼容性和源码检查；任何模式都不会启用扩展。'
}

die() {
    printf '错误：%s\n' "$*" >&2
    exit 1
}

require_normal_user() {
    [ "$(id -u)" -ne 0 ] || die '请在真实图形登录会话中以普通用户运行；不要使用 sudo/pkexec。'
    [ -n "${HOME:-}" ] || die 'HOME 未设置。'
}

check_compatibility() {
    require_normal_user

    [ -r /etc/os-release ] || die '无法读取 /etc/os-release。'
    # shellcheck disable=SC1091
    . /etc/os-release
    [ "${ID:-}" = ubuntu ] || die "仅验证了 Ubuntu 26.04；检测到 ${PRETTY_NAME:-unknown}。"
    [ "${VERSION_ID:-}" = 26.04 ] || die "仅验证了 Ubuntu 26.04；检测到 ${PRETTY_NAME:-unknown}。"

    case ${XDG_CURRENT_DESKTOP:-} in
        *GNOME* | *gnome*) ;;
        *) die "需要真实 GNOME 图形会话；XDG_CURRENT_DESKTOP=${XDG_CURRENT_DESKTOP:-unset}。" ;;
    esac
    [ "${XDG_SESSION_TYPE:-}" = wayland ] ||
        die "仅验证了 Wayland；XDG_SESSION_TYPE=${XDG_SESSION_TYPE:-unset}。"

    command -v gnome-shell >/dev/null 2>&1 || die '缺少 gnome-shell。'
    command -v gnome-extensions >/dev/null 2>&1 || die '缺少 gnome-extensions。'
    shell_version=$(gnome-shell --version | awk '{print $NF}')
    shell_major=${shell_version%%.*}
    [ "$shell_major" = 50 ] || die "扩展仅声明支持 GNOME Shell 50；检测到 $shell_version。"

    printf '%s\n' \
        "兼容性检查通过：${PRETTY_NAME:-Ubuntu 26.04}" \
        "GNOME Shell：$shell_version" \
        "会话：${XDG_CURRENT_DESKTOP:-unknown} / ${XDG_SESSION_TYPE:-unknown}"
}

check_source() {
    for file in $runtime_files; do
        [ -f "$source_dir/$file" ] || die "缺少扩展运行文件：$file。"
    done
    [ -x "$source_dir/tests/static-contract.sh" ] ||
        die '缺少可执行的扩展静态检查脚本。'
    command -v python3 >/dev/null 2>&1 || die '缺少 python3，无法验证扩展元数据。'
    command -v gjs >/dev/null 2>&1 || die '缺少 gjs，无法按 GNOME JavaScript 语法解析扩展。'

    python3 - "$source_dir/metadata.json" "$uuid" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
expected_uuid = sys.argv[2]
metadata = json.loads(path.read_text(encoding="utf-8"))
assert metadata.get("uuid") == expected_uuid, "metadata uuid mismatch"
assert metadata.get("version") == 4, "installation source must be extension version 4"
assert metadata.get("shell-version") == ["50"], "shell-version must be exactly ['50']"
assert metadata.get("session-modes") == ["user"], "extension must be user-session only"
PY

    "$source_dir/tests/static-contract.sh"
    printf '%s\n' '完整扩展协议、GNOME 50 API 与 JavaScript 静态检查通过。'
}

extension_is_enabled() {
    gnome-extensions list --enabled 2>/dev/null | grep -Fqx "$uuid"
}

require_extension_disabled() {
    if extension_is_enabled; then
        die "扩展当前已启用。请先运行 gnome-extensions disable '$uuid'，确认桌面稳定后再重试。"
    fi
}

print_change_plan() {
    printf '%s\n' '' '将原子替换以下用户级扩展目录及运行文件：'
    for file in $runtime_files; do
        printf '  %s/%s\n' "$target_dir" "$file"
    done
    printf '%s\n' \
        '不会修改或重启：GDM、PAM、GNOME 会话配置、systemd、桌面自启动。' \
        '不会启用扩展；启用必须在安装后由用户另行手动执行。'
}

make_stamp() {
    date +%Y%m%d-%H%M%S
}

install_extension() {
    check_compatibility
    check_source
    require_extension_disabled
    [ ! -L "$target_dir" ] || die "拒绝写入符号链接目标：$target_dir"

    stamp=$(make_stamp)
    backup_dir=$backup_root/$stamp-$$/$uuid
    print_change_plan
    if [ -e "$target_dir" ]; then
        printf '%s\n' \
            "现有扩展将先完整备份到：$backup_dir" \
            '应用前回滚命令：' \
            "  gnome-extensions disable '$uuid'" \
            "  '$script_path' --rollback '$backup_dir'"
    else
        printf '%s\n' \
            '当前没有旧扩展；应用前回滚命令：' \
            "  gnome-extensions disable '$uuid'" \
            "  '$script_path' --uninstall"
    fi

    mkdir -p "$target_root"
    candidate_dir=$(mktemp -d "$target_root/.three-finger-drag-install.XXXXXX")
    cleanup_candidate() {
        if [ -n "${candidate_dir:-}" ] && [ -d "$candidate_dir" ]; then
            rm -rf -- "$candidate_dir"
        fi
    }
    trap cleanup_candidate EXIT HUP INT TERM

    for file in $runtime_files; do
        install -m 0644 "$source_dir/$file" "$candidate_dir/$file"
    done
    # mktemp creates the directory as 0700. Set the final mode before replacing
    # the live path so there is no post-swap permission step that can fail.
    chmod 0755 "$candidate_dir"

    if [ -e "$target_dir" ]; then
        mkdir -p "$(dirname -- "$backup_dir")"
        mv -- "$target_dir" "$backup_dir"
        printf '已备份现有扩展：%s\n' "$backup_dir"
    fi

    if ! mv -- "$candidate_dir" "$target_dir"; then
        if [ -d "$backup_dir" ] && [ ! -e "$target_dir" ]; then
            mv -- "$backup_dir" "$target_dir"
        fi
        die '无法安装候选扩展；旧版本已恢复。'
    fi
    candidate_dir=
    trap - EXIT HUP INT TERM

    printf '%s\n' \
        "扩展源码已安装到：$target_dir" \
        '扩展仍处于未启用状态；本脚本没有更改 GNOME 扩展开关。' \
        '请按 LINUX_PORT.md 的“真实登录会话手动验证”逐项测试，再决定是否保留。'
}

rollback_extension() {
    require_normal_user
    [ "$#" -eq 1 ] || die '--rollback 需要一个备份目录。'
    require_extension_disabled
    [ ! -L "$target_dir" ] || die "拒绝替换符号链接目标：$target_dir"

    candidate=$1
    [ -d "$candidate" ] || die "备份目录不存在：$candidate"
    backup_dir=$(CDPATH='' cd -- "$candidate" && pwd -P)
    backup_root_real=$(CDPATH='' cd -- "$backup_root" 2>/dev/null && pwd -P) ||
        die "备份根目录不存在：$backup_root"
    case $backup_dir/ in
        "$backup_root_real"/*) ;;
        *) die "拒绝使用备份根目录之外的路径：$backup_dir" ;;
    esac
    [ -f "$backup_dir/metadata.json" ] || die '备份中缺少 metadata.json。'
    check_uuid=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["uuid"])' "$backup_dir/metadata.json")
    [ "$check_uuid" = "$uuid" ] || die "备份 UUID 不匹配：$check_uuid"

    stamp=$(make_stamp)
    displaced_dir=$backup_root/rollback-displaced-$stamp-$$/$uuid
    mkdir -p "$target_root"
    candidate_dir=$(mktemp -d "$target_root/.three-finger-drag-rollback.XXXXXX")
    cleanup_candidate() {
        if [ -n "${candidate_dir:-}" ] && [ -d "$candidate_dir" ]; then
            rm -rf -- "$candidate_dir"
        fi
    }
    trap cleanup_candidate EXIT HUP INT TERM
    cp -a -- "$backup_dir/." "$candidate_dir/"
    chmod 0755 "$candidate_dir"

    printf '%s\n' \
        "将恢复：$backup_dir" \
        "到：$target_dir" \
        "当前版本（如存在）将先移到：$displaced_dir" \
        '扩展不会被启用。'

    if [ -e "$target_dir" ]; then
        mkdir -p "$(dirname -- "$displaced_dir")"
        mv -- "$target_dir" "$displaced_dir"
    fi
    if ! mv -- "$candidate_dir" "$target_dir"; then
        if [ -d "$displaced_dir" ] && [ ! -e "$target_dir" ]; then
            mv -- "$displaced_dir" "$target_dir"
        fi
        die '无法原子恢复备份；当前版本已恢复。'
    fi
    candidate_dir=
    trap - EXIT HUP INT TERM
    printf '%s\n' '旧版本已恢复。请注销并重新登录后再做验证。'
}

uninstall_extension() {
    require_normal_user
    require_extension_disabled
    [ ! -L "$target_dir" ] || die "拒绝移动符号链接目标：$target_dir"

    if [ ! -e "$target_dir" ]; then
        printf '%s\n' '扩展目录不存在，无需卸载。'
        return
    fi

    stamp=$(make_stamp)
    removed_dir=$backup_root/removed-$stamp-$$/$uuid
    printf '%s\n' \
        "将把扩展移到可恢复位置：$removed_dir" \
        '不会删除备份，也不会修改其他 GNOME 配置。'
    mkdir -p "$(dirname -- "$removed_dir")"
    mv -- "$target_dir" "$removed_dir"
    printf '%s\n' '扩展文件已移出加载路径。请注销并重新登录。'
}

action=${1:---check}
case $action in
    --check)
        [ "$#" -eq 1 ] || die '--check 不接受其他参数。'
        check_compatibility
        check_source
        print_change_plan
        printf '%s\n' '只读检查完成；没有修改任何文件或扩展开关。'
        ;;
    --install)
        [ "$#" -eq 1 ] || die '--install 不接受其他参数。'
        install_extension
        ;;
    --rollback)
        shift
        rollback_extension "$@"
        ;;
    --uninstall)
        [ "$#" -eq 1 ] || die '--uninstall 不接受其他参数。'
        uninstall_extension
        ;;
    --help | -h)
        usage
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
