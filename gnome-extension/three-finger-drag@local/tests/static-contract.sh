#!/bin/sh
set -eu

extension_dir=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
repo_root=$(CDPATH='' cd -- "$extension_dir/../.." && pwd)

python3 -c 'import json, sys; json.load(open(sys.argv[1], encoding="utf-8"))' \
    "$extension_dir/metadata.json"
python3 -c 'import json, sys; assert json.load(open(sys.argv[1], encoding="utf-8"))["version"] == 6' \
    "$extension_dir/metadata.json"
python3 -c 'import sys, xml.etree.ElementTree as ET; ET.parse(sys.argv[1])' \
    "$extension_dir/interface.xml"
grep -Fq "4 | 5) installed_runtime_files=\$runtime_files" \
    "$repo_root/scripts/verify-linux-session.sh"
grep -Fq "6) installed_runtime_files=\$runtime_files" \
    "$repo_root/scripts/verify-linux-session.sh"

gjs -m "$extension_dir/tests/protocol-contract.js"
gjs -m "$extension_dir/tests/input-ownership-contract.js"
gjs -m "$extension_dir/tests/exact-three-contract.js"
gjs -m "$extension_dir/tests/four-finger-down-contract.js"
GI_TYPELIB_PATH=/usr/lib/x86_64-linux-gnu/mutter-18:/usr/lib/gnome-shell \
LD_LIBRARY_PATH=/usr/lib/x86_64-linux-gnu/mutter-18:/usr/lib/gnome-shell \
    gjs -m "$extension_dir/tests/gnome-api-contract.js"
GI_TYPELIB_PATH=/usr/lib/x86_64-linux-gnu/mutter-18:/usr/lib/gnome-shell \
LD_LIBRARY_PATH=/usr/lib/x86_64-linux-gnu/mutter-18:/usr/lib/gnome-shell \
    gjs -m "$extension_dir/tests/window-state-contract.js"

if command -v gdbus-codegen >/dev/null 2>&1; then
    gdbus-codegen --interface-info-body --output /dev/null \
        "$extension_dir/interface.xml"
fi

for module in "$extension_dir"/*.js; do
    output_file=$(mktemp)
    if GI_TYPELIB_PATH=/usr/lib/x86_64-linux-gnu/mutter-18:/usr/lib/gnome-shell \
        LD_LIBRARY_PATH=/usr/lib/x86_64-linux-gnu/mutter-18:/usr/lib/gnome-shell \
        gjs -m "$module" >"$output_file" 2>&1; then
        :
    elif grep -Fq 'SyntaxError' "$output_file"; then
        sed -n '1,80p' "$output_file" >&2
        rm -f -- "$output_file"
        exit 1
    fi
    rm -f -- "$output_file"
done

grep -Fq "get_touchpad_gesture_finger_count()" "$extension_dir/extension.js"
grep -Fq "this._owned = fingers === 3" "$extension_dir/exactThreeGuard.js"
if grep -Fq "fingers >= 3" "$extension_dir/extension.js"; then
    exit 1
fi
grep -Fq "twoFinger: true" "$extension_dir/protocol.js"
grep -Fq "fiveFinger: false" "$extension_dir/protocol.js"
grep -Fq "minimizeAll: true" "$extension_dir/protocol.js"
grep -Fq "axisResize: true" "$extension_dir/protocol.js"
grep -Fq "pinch: true" "$extension_dir/protocol.js"
grep -Fq "close: true" "$extension_dir/protocol.js"
grep -Fq "dynamicWorkspace: true" "$extension_dir/protocol.js"
grep -Fq "moveCursor: true" "$extension_dir/protocol.js"
grep -Fq "livePreview: true" "$extension_dir/protocol.js"
grep -Fq "snapPreview: true" "$extension_dir/protocol.js"
grep -Fq "adaptiveSnapAnimation: true" "$extension_dir/protocol.js"
grep -Fq "appSwitch: true" "$extension_dir/protocol.js"
grep -Fq "ConfigureAsync" "$extension_dir/broker.js"
grep -Fq "class CapabilityBroker" "$extension_dir/broker.js"
grep -Fq "GetCapabilities" "$extension_dir/broker.js"
grep -Fq "new WindowBackend" "$extension_dir/broker.js"
if grep -Fq 'this._target.is_alive()' "$extension_dir/windowBackend.js"; then
    printf '%s\n' 'window lifetime must use unmanaging, not Meta.Window.is_alive()' >&2
    exit 1
fi
grep -Fq "new GestureHud" "$extension_dir/broker.js"
grep -Fq "this._backend?.cancel()" "$extension_dir/broker.js"
grep -Fq "sequence is not strictly increasing" "$extension_dir/broker.js"
if grep -Eq 'arbitrate[A-Za-z]+\(' "$extension_dir/extension.js"; then
    exit 1
fi
if grep -Fq "cancelForFourFingerGesture" "$extension_dir/extension.js"; then
    exit 1
fi
grep -Fq "TOUCHPAD_PINCH" "$extension_dir/extension.js"
grep -Fq "TOUCHPAD_HOLD" "$extension_dir/extension.js"
if grep -Eq 'InputOwnership|resource:///org/gnome/shell/ui/main' \
    "$extension_dir/extension.js" "$extension_dir/broker.js"; then exit 1; fi
grep -Fq "this._mutated = true" "$extension_dir/windowBackend.js"
grep -Fq "A boundary is an accepted no-op" "$extension_dir/windowBackend.js"
grep -Fq "case 'pinchOut'" "$extension_dir/windowBackend.js"
grep -Fq "case 'pinchIn'" "$extension_dir/windowBackend.js"
grep -Fq "this._target.delete(global.get_current_time())" "$extension_dir/windowBackend.js"
grep -Fq "append_new_workspace(false" "$extension_dir/windowBackend.js"
grep -Fq "target.set_maximize_flags(flags)" "$extension_dir/windowBackend.js"
grep -Fq "target.move_to_monitor(target.get_monitor())" "$extension_dir/windowBackend.js"
grep -Fq "Meta.MaximizeFlags.BOTH" "$extension_dir/windowBackend.js"
grep -Fq "this._target.maximize()" "$extension_dir/windowBackend.js"
grep -Fq "MAXIMIZED_SETTLE_ATTEMPTS" "$extension_dir/windowBackend.js"
grep -Fq "rectMatches(target.get_frame_rect(), rounded)" "$extension_dir/windowBackend.js"
grep -Fq "actorVisualRect" "$extension_dir/windowBackend.js"
grep -Fq "Clutter.AnimationMode.EASE_OUT_CUBIC" "$extension_dir/windowBackend.js"
grep -Fq "Clutter.AnimationMode.EASE_OUT_QUART" "$extension_dir/windowBackend.js"
grep -Fq "adaptiveSnapDuration" "$extension_dir/windowBackend.js"
grep -Fq "showSnapPreview" "$extension_dir/hud.js"
grep -Fq "snapPreviewRect" "$extension_dir/hud.js"
grep -Fq "{width: 68, height: 44}" "$extension_dir/hud.js"
grep -Fq "{width: 96, height: 62}" "$extension_dir/hud.js"
grep -Fq "this._hideActor(this._previewActor, immediate, 100)" "$extension_dir/hud.js"
grep -Fq "Main.wm?.skipNextEffect?.(actor)" "$extension_dir/hud.js"
if grep -Fq "moveResizeTarget(target, interpolateRect" "$extension_dir/windowBackend.js"; then
    printf '%s\n' 'snap animation must not stream per-frame window geometry writes' >&2
    exit 1
fi
if grep -Fq "this._hud.show(" "$extension_dir/windowBackend.js"; then
    printf '%s\n' 'large-area HUD calls must not remain in the window backend' >&2
    exit 1
fi
if grep -Eq 'show\(rect|createWindowClone|paint_to_content' \
    "$extension_dir/hud.js" "$extension_dir/windowBackend.js"; then
    printf '%s\n' 'large preview and snapshot-clone rendering must stay removed' >&2
    exit 1
fi
grep -Fq "backend?.destroy()" "$extension_dir/broker.js"
grep -Fq "minimizeAllOnActiveWorkspace" "$extension_dir/broker.js"
grep -Fq "get_gesture_motion_delta_unaccelerated()" "$extension_dir/extension.js"
grep -Fq "Clutter.EVENT_PROPAGATE" "$extension_dir/extension.js"
grep -Fq "import St from 'gi://St'" "$extension_dir/hud.js"
grep -Fq "Main.layoutManager.addTopChrome" "$extension_dir/hud.js"
grep -Fq "connect('destroy'" "$extension_dir/hud.js"
grep -Fq "finally {" "$extension_dir/broker.js"
grep -Fq "warp_pointer(point.x, point.y)" "$extension_dir/windowBackend.js"
grep -Fq "cursorAfterWindowMove" "$extension_dir/windowBackend.js"
grep -Fq "this._queueFrame({kind: 'preview', event})" "$extension_dir/windowBackend.js"
grep -Fq "takeRebaselineRequest" "$extension_dir/windowBackend.js"
grep -Fq "new GLib.Variant('(bs)'" "$extension_dir/broker.js"
grep -Fq 'name="rebaseline" type="s" direction="out"' "$extension_dir/interface.xml"
if grep -Eq 'nativeTitlebarDrag|genuine CSD' "$extension_dir/windowBackend.js"; then
    printf '%s\n' 'two-finger swipes must not be implemented as native pointer drags' >&2
    exit 1
fi
grep -Fq "class InputOwnership" "$extension_dir/inputOwnership.js"
grep -Fq "class ExactThreeGuardSet" "$extension_dir/exactThreeGuard.js"
grep -Fq "event.get_source_device()" "$extension_dir/extension.js"
grep -Fq "contains a forbidden target identifier" "$extension_dir/protocol.js"
grep -Fq "window.get_gtk_application_id()" "$extension_dir/windowBackend.js"
if grep -Eq 'get_pid\(|windowId|targetId|\.kill\(' "$extension_dir/windowBackend.js"; then
    exit 1
fi
if grep -REq 'autostart|systemd|gnome-extensions enable' "$extension_dir"/*.js; then
    exit 1
fi

printf '%s\n' 'GNOME broker static contract: ok'
