/** Decides exact-three ownership on BEGIN and keeps it until the terminal event. */
export class ExactThreeGuard {
    constructor() {
        this.reset();
    }

    get owned() {
        return this._owned;
    }

    decide(phase, fingers) {
        const begin = phase === 'begin';
        const terminal = phase === 'end' || phase === 'cancel';
        if (begin && !this._active) {
            this._active = true;
            this._owned = fingers === 3;
        }
        const stop = this._owned;
        if (terminal) {
            this._owned = false;
            this._active = false;
        }
        return stop;
    }

    reset() {
        this._owned = false;
        this._active = false;
    }
}

/** Keeps exact-three ownership independent for every physical touchpad. */
export class ExactThreeGuardSet {
    constructor() {
        this._guards = new Map();
    }

    decide(device, family, phase, fingers) {
        if (device === null || device === undefined)
            return false;
        let deviceGuards = this._guards.get(device);
        if (!deviceGuards) {
            deviceGuards = new Map();
            this._guards.set(device, deviceGuards);
        }
        let guard = deviceGuards.get(family);
        if (!guard) {
            guard = new ExactThreeGuard();
            deviceGuards.set(family, guard);
        }
        const stop = guard.decide(phase, fingers);
        if (phase === 'end' || phase === 'cancel') {
            deviceGuards.delete(family);
            if (deviceGuards.size === 0)
                this._guards.delete(device);
        }
        return stop;
    }

    reset() {
        this._guards.clear();
    }
}
