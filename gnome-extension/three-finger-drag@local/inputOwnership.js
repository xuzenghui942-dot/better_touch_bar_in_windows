export const StreamOwner = Object.freeze({
    IDLE: 'idle',
    GNOME: 'gnome',
    BROKER: 'broker',
});

/**
 * Tracks compositor input ownership independently from the window transaction.
 * Once the broker stops the first event, every event through END/CANCEL is
 * stopped even if D-Bus, the target, or the configured owner disappears.
 */
export class InputOwnership {
    constructor() {
        this.reset();
    }

    get twoOwner() {
        return this._twoOwner;
    }

    get swipeOwner() {
        return this._fiveOwner;
    }

    get fiveOwner() {
        return this._fiveOwner;
    }

    get twoFamily() {
        return this._twoFamily;
    }

    decideTwoFinger(family, phase, canClaim) {
        const terminal = phase === 'end' || phase === 'cancel';
        let claimed = false;
        if (this._twoOwner === StreamOwner.IDLE && !terminal) {
            this._twoOwner = canClaim ? StreamOwner.BROKER : StreamOwner.GNOME;
            this._twoFamily = family;
            claimed = canClaim;
        }
        const draining = this._twoFamily === 'draining';
        const stop = this._twoOwner === StreamOwner.BROKER;
        const endedOwnedStream = terminal && stop;
        let drainStarted = false;
        let released = false;
        if (terminal && this._twoOwner !== StreamOwner.IDLE) {
            if (family === 'hold' && phase === 'cancel' && !draining) {
                // libinput often emits HOLD CANCEL immediately before SCROLL
                // or PINCH begins. Keep ownership during a short drain window
                // so the successor cannot leak to the client.
                this._twoFamily = 'draining';
                drainStarted = true;
            } else {
                this._twoOwner = StreamOwner.IDLE;
                this._twoFamily = null;
                released = true;
            }
        }
        return {stop, claimed, endedOwnedStream, drainStarted, released};
    }

    releaseTwoFingerDrain() {
        if (this._twoFamily !== 'draining')
            return false;
        this._twoOwner = StreamOwner.IDLE;
        this._twoFamily = null;
        return true;
    }

    decideFiveFinger(family, phase, fingers, canClaimFive) {
        const isBegin = phase === 'begin';
        const isEnd = phase === 'end' || phase === 'cancel';
        let claimed = false;
        let cancelTransaction = false;

        if (this._fiveOwner === StreamOwner.IDLE && !isEnd) {
            if (fingers === 5 && canClaimFive) {
                // Five-finger native ownership is only acquired on a real
                // compositor BEGIN. UPDATE must never steal a stream which
                // GNOME already started as four fingers.
                this._fiveOwner = isBegin ? StreamOwner.BROKER : StreamOwner.GNOME;
                claimed = true;
            } else {
                this._fiveOwner = StreamOwner.GNOME;
            }
            this._fiveFamily = family;
        }

        if (this._fiveOwner === StreamOwner.BROKER && fingers !== 5)
            cancelTransaction = true;

        const draining = this._fiveFamily === 'draining';
        const stop = this._fiveOwner === StreamOwner.BROKER;
        const endedOwnedStream = isEnd && stop;
        let drainStarted = false;
        let released = false;
        if (isEnd && this._fiveOwner !== StreamOwner.IDLE) {
            if (family === 'hold' && phase === 'cancel' && !draining) {
                // A libinput HOLD cancellation commonly hands the same
                // physical contacts directly to PINCH or SWIPE. Preserve the
                // original owner across that transition.
                this._fiveFamily = 'draining';
                drainStarted = true;
            } else {
                this._fiveOwner = StreamOwner.IDLE;
                this._fiveFamily = null;
                released = true;
            }
        }
        return {
            stop,
            claimed,
            cancelTransaction,
            endedOwnedStream,
            drainStarted,
            released,
        };
    }

    releaseFiveFingerDrain() {
        if (this._fiveFamily !== 'draining')
            return false;
        this._fiveOwner = StreamOwner.IDLE;
        this._fiveFamily = null;
        return true;
    }

    decideTouchpadSwipe({phase, fingers, canClaimFive}) {
        return this.decideFiveFinger('swipe', phase, fingers, canClaimFive);
    }

    reset() {
        this._twoOwner = StreamOwner.IDLE;
        this._twoFamily = null;
        this._fiveOwner = StreamOwner.IDLE;
        this._fiveFamily = null;
    }
}
