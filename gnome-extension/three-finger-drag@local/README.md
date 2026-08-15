# GNOME 50 exact-three guard and two-finger window broker

This is a user-session-only GNOME Shell 50 extension. It is not a system
service and it never enables itself. Install it with the repository's staged
script, keep advanced gestures off for the first real-session test, and retain
the printed TTY rollback command.

The extension owns exact-three touchpad gesture streams per physical device so
GNOME's `>= 3` workspace gesture cannot duplicate the application's
three-finger drag. Independent four-finger streams remain GNOME-owned. When
advanced mode and its single four-finger option are enabled, the extension
passively recognizes a completed physical downward swipe and then minimizes
all eligible normal windows on the active workspace. It never stops that
four-finger stream, and left/right/up remain unchanged. If an
already-owned three-finger stream gains a fourth finger, that physical stream
stays stopped through END/CANCEL; handing half a stream to GNOME could leave
its SwipeTracker stuck.

Mutter can deliver native SCROLL/PINCH/SWIPE/HOLD to a focused Wayland client
before a Shell capture handler runs. The broker therefore does not claim
two-finger ownership in Clutter. When the user explicitly enables advanced
mode, the Rust backend first clones the physical touchpad through uinput and
then uses EVIOCGRAB on the physical device. Normal pointer/buttons and
non-owned gestures are replayed through that clone. Every standalone
one-finger frame is forwarded immediately and never requests a titlebar probe.
Only after exactly two contacts appear does Rust buffer at most 64 frames/50 ms
and request a private probe. A rejection, timeout, overflow, return to one
finger, third/fourth contact or neutral end replays the buffered continuation
exactly once and then emits the boundary frame, preserving continuous native
input without a synthetic click.

The capability response declares `twoFinger=true`, `fiveFinger=false`, exact
three true, and four false, plus passive action `minimizeAll=true`. Four remains
false because the broker never claims native four-finger input ownership.
Five-finger configuration is rejected by the Linux application and is not
exposed in its UI.

One unique session-bus sender must complete `Configure` with protocol version
2, extension version 6, the matching input-proxy generation and an explicitly
enabled configuration. `Configure(false)` clears gesture
settings while retaining that sender's channel ownership until it disconnects.
The D-Bus interface is `io.github.xuzenghui942.ThreeFingerDrag.Gnome1`, exported
at `/io/github/xuzenghui942/ThreeFingerDrag/Gnome` under bus name
`io.github.xuzenghui942.ThreeFingerDrag.Gnome`. Requests use strictly
increasing sequences and one global transaction; idle/max-duration watchdogs,
sender loss, malformed input and target destruction cancel and restore.

`ProbeTarget` returns a caller-bound, single-use target token that expires in
250 ms. `Begin` consumes that token and revalidates both the pointer band and
the exact private `Meta.Window`; neither method accepts or exposes a PID,
window ID or target ID. The extension selects a manageable normal window under
the pointer's conservative top-frame band. The
implemented two-finger actions are half/quarter snapping,
maximize/minimize, graceful close (Meta.Window.delete only), down-action
selection, existing/dynamic workspaces, adjacent monitors, pinch
maximize/restore, hold application switching and cancel restoration.
The Linux settings surface intentionally keeps axis resizing, live window
preview and cursor-follow disabled. Directional swipes never synthesize a
left-button drag, never move the pointer with the fingers and never draw a
screen-sized destination HUD; half/quarter directions instead show a compact
pointer-adjacent screen map, and the real window moves only after release.
Those six destinations use a 180–260 ms adaptive, no-overshoot compositor
transition that can retarget from its live presentation rectangle. Maximize,
minimize and the other compact chooser, workspace, monitor and application HUD
paths retain their existing behavior. Five-finger free move/resize remains
unavailable.

Run offline checks from the repository root:

```sh
gnome-extension/three-finger-drag@local/tests/static-contract.sh
```

They validate JSON/XML, D-Bus signatures, protocol/geometry fixtures,
exact-three ownership, required GNOME 50 GI methods and GJS syntax. They do not
load or enable the extension and cannot replace a real touchpad test followed
by logout and fresh login.
