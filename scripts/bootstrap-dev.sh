#!/bin/sh
set -eu

system_packages='libwebkit2gtk-4.1-dev build-essential libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev patchelf shellcheck'
expected_zig=0.16.0
failures=0

usage() {
    printf '%s\n' \
        "用法：$0 [--check|--install-system]" \
        '' \
        '--check          只验证现有开发环境（默认）' \
        '--install-system 通过 apt 安装 Ubuntu 官方构建依赖' \
        '' \
        '此脚本绝不安装/启用 GNOME 扩展、udev 规则、服务或自启动项。'
}

fail() {
    printf '失败：%s\n' "$*" >&2
    failures=$((failures + 1))
}

check_os() {
    [ -r /etc/os-release ] || {
        fail '无法读取 /etc/os-release。'
        return
    }
    # shellcheck disable=SC1091
    . /etc/os-release
    [ "${ID:-}" = ubuntu ] && [ "${VERSION_ID:-}" = 26.04 ] ||
        fail "开发环境仅验证了 Ubuntu 26.04；检测到 ${PRETTY_NAME:-unknown}。"
}

missing_packages() {
    for package in $system_packages; do
        status=$(dpkg-query -W -f='${Status}' "$package" 2>/dev/null || true)
        [ "$status" = 'install ok installed' ] || printf '%s\n' "$package"
    done
}

check_packages() {
    missing=$(missing_packages)
    if [ -n "$missing" ]; then
        fail "缺少 Ubuntu 构建依赖：$(printf '%s' "$missing" | tr '\n' ' ')"
    else
        printf '%s\n' "Ubuntu 构建依赖齐全：$system_packages"
    fi
}

install_system_packages() {
    [ "$(id -u)" -ne 0 ] || {
        fail '请以普通用户运行；脚本只对 apt 子命令使用 sudo。'
        return
    }
    command -v sudo >/dev/null 2>&1 || {
        fail '缺少 sudo。'
        return
    }

    printf '%s\n' \
        "将从 Ubuntu 官方仓库安装：$system_packages" \
        '不会触碰 GNOME 扩展、udev、GDM、PAM、systemd 或自启动。'
    sudo apt-get update
    # shellcheck disable=SC2086
    sudo apt-get install --no-install-recommends $system_packages
}

check_toolchains() {
    PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
    export PATH

    if command -v rustc >/dev/null 2>&1; then
        rust_version=$(rustc --version)
        printf 'Rust：%s\n' "$rust_version"
    else
        fail "未找到用户级 Rust：$HOME/.cargo/bin/rustc"
    fi

    if command -v cargo >/dev/null 2>&1; then
        cargo --version
    else
        fail '未找到 cargo。'
    fi
    if command -v rustfmt >/dev/null 2>&1; then
        rustfmt --version
    else
        fail '未安装 rustfmt。'
    fi
    if command -v cargo-clippy >/dev/null 2>&1; then
        cargo clippy --version
    else
        fail '未安装 clippy。'
    fi
    if tauri_version=$(cargo tauri --version 2>/dev/null); then
        printf 'Tauri CLI：%s\n' "$tauri_version"
        case $tauri_version in
            tauri-cli\ 2.*) ;;
            *) fail "需要 tauri-cli 2.x，当前为 $tauri_version。" ;;
        esac
    else
        fail '未安装 tauri-cli 2.x（cargo tauri）。'
    fi

    expected_zig_path=$HOME/.local/opt/zig-x86_64-linux-$expected_zig/zig
    if [ -x "$expected_zig_path" ]; then
        zig_version=$($expected_zig_path version)
        printf 'Zig：%s (%s)\n' "$zig_version" "$expected_zig_path"
        [ "$zig_version" = "$expected_zig" ] || fail "需要 Zig $expected_zig。"
    else
        fail "未找到 Zig fallback：$expected_zig_path"
    fi

    if command -v cc >/dev/null 2>&1; then
        printf '系统 C 编译器：%s\n' "$(command -v cc)"
    elif [ -x "$HOME/.local/bin/zig-cc" ]; then
        printf '系统 cc 不可用；可显式使用 fallback：CC=%s\n' "$HOME/.local/bin/zig-cc"
    else
        fail '系统 cc 与 ~/.local/bin/zig-cc 均不可用。'
    fi
}

action=${1:---check}
[ "$#" -le 1 ] || {
    usage >&2
    exit 2
}
case $action in
    --check) ;;
    --install-system) install=yes ;;
    --help | -h)
        usage
        exit 0
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac

check_os
if [ "${install:-no}" = yes ]; then
    install_system_packages
fi
check_packages
check_toolchains

if [ "$failures" -ne 0 ]; then
    printf '开发环境失败项：%s\n' "$failures" >&2
    exit 1
fi
printf '%s\n' '开发环境检查通过；没有修改任何桌面集成。'
