import Clutter from 'gi://Clutter';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

import {CapabilityBroker} from './broker.js';
import {ExactThreeGuardSet} from './exactThreeGuard.js';

export default class ThreeFingerDragGestureGuard extends Extension {
    enable() {
        if (this._capturedEventId !== undefined)
            return;

        this._exactThreeGuard = new ExactThreeGuardSet();

        // GNOME 50's SwipeTracker accepts every touchpad swipe with at least
        // three fingers.  Stop only the three-finger variant in the capture
        // phase, before SwipeTracker's event::touchpad handlers see it. The
        // optional Rust input proxy independently owns only explicitly enabled
        // advanced two-finger streams; this guard never arbitrates them.
        this._capturedEventId = global.stage.connect(
            'captured-event::touchpad',
            (_actor, event) => {
                if (event.type() === Clutter.EventType.TOUCHPAD_PINCH ||
                    event.type() === Clutter.EventType.TOUCHPAD_HOLD) {
                    const fingers = event.get_touchpad_gesture_finger_count();
                    const phase = gesturePhaseName(event.get_gesture_phase());
                    // Stop exact-three compositor gestures consistently. Do
                    // not advertise or claim two/five-finger exclusivity: on
                    // Wayland Mutter may deliver those to the focused client
                    // before this Shell capture handler runs.
                    const exactThreeStop = this._exactThreeGuard.decide(
                        sourceDevice(event), touchpadFamily(event), phase, fingers);
                    if (exactThreeStop)
                        return Clutter.EVENT_STOP;
                    return Clutter.EVENT_PROPAGATE;
                }

                if (event.type() !== Clutter.EventType.TOUCHPAD_SWIPE)
                    return Clutter.EVENT_PROPAGATE;

                const fingers = event.get_touchpad_gesture_finger_count();
                const phase = event.get_gesture_phase();
                const phaseName = gesturePhaseName(phase);
                const exactThreeStop = this._exactThreeGuard.decide(
                    sourceDevice(event), 'swipe', phaseName, fingers);
                if (exactThreeStop)
                    return Clutter.EVENT_STOP;
                let dx = 0;
                let dy = 0;
                try {
                    [dx, dy] = event.get_gesture_motion_delta_unaccelerated();
                } catch (_error) {
                    // Passive recognition fails open if Mutter cannot expose
                    // a delta for this event.
                }
                this._broker?.observeFourFingerSwipe(
                    sourceDevice(event), phaseName, fingers, dx, dy);
                return Clutter.EVENT_PROPAGATE;
            });

        // The exact-three guard above remains useful even when the optional
        // D-Bus broker cannot start. Advanced actions fail closed in that case.
        try {
            const [loaded, contents] = this.dir.get_child('interface.xml')
                .load_contents(null);
            if (!loaded)
                throw new Error('cannot read interface.xml');
            const interfaceXml = new TextDecoder().decode(contents);
            this._broker = new CapabilityBroker(interfaceXml);
        } catch (error) {
            console.error(`${this.uuid}: capability endpoint unavailable: ${error.message}`);
            this._broker?.destroy();
            this._broker = null;
        }
    }

    disable() {
        if (this._capturedEventId !== undefined) {
            global.stage.disconnect(this._capturedEventId);
            this._capturedEventId = undefined;
        }
        this._broker?.destroy();
        this._broker = null;
        this._exactThreeGuard?.reset();
        this._exactThreeGuard = null;
    }
}

function gesturePhaseName(phase) {
    switch (phase) {
    case Clutter.TouchpadGesturePhase.BEGIN:
        return 'begin';
    case Clutter.TouchpadGesturePhase.END:
        return 'end';
    case Clutter.TouchpadGesturePhase.CANCEL:
        return 'cancel';
    default:
        return 'update';
    }
}

function sourceDevice(event) {
    try {
        return event.get_source_device();
    } catch (_error) {
        return null;
    }
}

function touchpadFamily(event) {
    return event.type() === Clutter.EventType.TOUCHPAD_HOLD ? 'hold' : 'pinch';
}
