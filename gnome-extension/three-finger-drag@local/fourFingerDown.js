const MIN_DOWN_DISTANCE = 72;
const DIRECTION_DOMINANCE = 1.25;

/**
 * Passively recognizes one complete, exact-four-finger downward swipe.
 *
 * It never claims or stops the Clutter stream. That keeps GNOME's ownership
 * intact for left, right and up, and avoids stealing a gesture after GNOME has
 * already received BEGIN.
 */
export class FourFingerDownGesture {
    constructor() {
        this.reset();
    }

    observe(phase, fingers, dx = 0, dy = 0) {
        if (phase === 'begin') {
            this._active = fingers === 4;
            this._dx = 0;
            this._dy = 0;
        }

        if (!this._active)
            return false;

        if (fingers !== 4) {
            this._active = false;
            return false;
        }

        if (Number.isFinite(dx))
            this._dx += dx;
        if (Number.isFinite(dy))
            this._dy += dy;

        if (phase === 'cancel') {
            this.reset();
            return false;
        }
        if (phase !== 'end')
            return false;

        const commit = this._dy >= MIN_DOWN_DISTANCE &&
            this._dy >= Math.abs(this._dx) * DIRECTION_DOMINANCE;
        this.reset();
        return commit;
    }

    reset() {
        this._active = false;
        this._dx = 0;
        this._dy = 0;
    }
}

/** Keeps passive recognition independent for every physical touchpad. */
export class FourFingerDownGestureSet {
    constructor() {
        this._gestures = new Map();
    }

    observe(device, phase, fingers, dx = 0, dy = 0) {
        if (device === null || device === undefined)
            return false;
        let gesture = this._gestures.get(device);
        if (!gesture) {
            gesture = new FourFingerDownGesture();
            this._gestures.set(device, gesture);
        }
        const commit = gesture.observe(phase, fingers, dx, dy);
        if (phase === 'end' || phase === 'cancel')
            this._gestures.delete(device);
        return commit;
    }

    reset() {
        this._gestures.clear();
    }
}
