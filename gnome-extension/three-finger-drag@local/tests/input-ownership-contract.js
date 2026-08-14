import {InputOwnership, StreamOwner} from '../inputOwnership.js';

// Clutter-first: the first stopped two-finger event owns the whole stream.
let ownership = new InputOwnership();
let decision = ownership.decideTwoFinger('scroll', 'update', true);
assert(decision.stop && decision.claimed, 'Clutter-first two-finger claim failed');
decision = ownership.decideTwoFinger('scroll', 'update', false); // transaction timed out
assert(decision.stop, 'timeout must not leak a half-owned two-finger stream');
decision = ownership.decideTwoFinger('scroll', 'end', false);
assert(decision.stop && ownership.twoOwner === StreamOwner.IDLE,
    'owned two-finger END must be stopped, then released');

// Begin-first only locks a window. It must stay IDLE until the first real
// compositor event atomically claims and stops the native stream.
ownership = new InputOwnership();
decision = ownership.decideTwoFinger('pinch', 'begin', true);
assert(decision.stop && decision.claimed,
    'Begin-first target was not adopted by the first compositor event');

// If Begin-first is cancelled before Clutter emits anything, InputOwnership is
// untouched. The next ordinary scroll and four-finger stream must propagate.
ownership = new InputOwnership();
assert(ownership.twoOwner === StreamOwner.IDLE &&
    ownership.swipeOwner === StreamOwner.IDLE,
'a window-only Begin must not preclaim native ownership');
assert(!ownership.decideTwoFinger('scroll', 'update', false).stop,
    'a client scroll after pre-compositor cancel was swallowed');
ownership.decideTwoFinger('scroll', 'end', false);
assert(!ownership.decideTouchpadSwipe({
    phase: 'begin', fingers: 4, canClaimFive: false,
}).stop, 'four-finger BEGIN after pre-compositor cancel was swallowed');
ownership.decideTouchpadSwipe({phase: 'end', fingers: 4, canClaimFive: false});

// A normal client-area scroll stays GNOME-owned for its entire stream.
ownership = new InputOwnership();
assert(!ownership.decideTwoFinger('scroll', 'update', false).stop,
    'client scroll was swallowed');
assert(!ownership.decideTwoFinger('scroll', 'update', true).stop,
    'mid-stream pointer move must not steal a GNOME-owned scroll');
ownership.decideTwoFinger('scroll', 'end', true);

// HOLD CANCEL may transition directly into PINCH/SCROLL without fingers lifting.
// Retain broker ownership through that boundary, then release on the successor.
ownership = new InputOwnership();
decision = ownership.decideTwoFinger('hold', 'begin', true);
assert(decision.stop && decision.claimed, 'two-finger hold claim failed');
decision = ownership.decideTwoFinger('hold', 'cancel', false);
assert(decision.stop && decision.drainStarted &&
    ownership.twoFamily === 'draining', 'hold cancel did not enter owned drain');
decision = ownership.decideTwoFinger('pinch', 'begin', false);
assert(decision.stop, 'hold-to-pinch transition leaked to the client');
decision = ownership.decideTwoFinger('pinch', 'end', false);
assert(decision.stop && ownership.twoOwner === StreamOwner.IDLE,
    'owned pinch terminal did not release the stream');

ownership = new InputOwnership();
ownership.decideTwoFinger('hold', 'begin', true);
ownership.decideTwoFinger('hold', 'cancel', false);
assert(ownership.releaseTwoFingerDrain() &&
    ownership.twoOwner === StreamOwner.IDLE,
'orphaned hold drain watchdog could not release ownership');

// Five fingers may only claim on BEGIN. Four -> five never steals GNOME's END.
ownership = new InputOwnership();
decision = ownership.decideTouchpadSwipe({
    phase: 'begin', fingers: 4, canClaimFive: false,
});
assert(!decision.stop, 'four-finger BEGIN must remain GNOME-owned');
decision = ownership.decideTouchpadSwipe({
    phase: 'update', fingers: 5, canClaimFive: true,
});
assert(!decision.stop, 'four-to-five transition stole a GNOME-owned stream');

// Five -> four cancels the window but retains native ownership through END.
ownership = new InputOwnership();
decision = ownership.decideTouchpadSwipe({
    phase: 'begin', fingers: 5, canClaimFive: true,
});
assert(decision.stop && decision.claimed, 'five-finger BEGIN claim failed');
decision = ownership.decideTouchpadSwipe({
    phase: 'update', fingers: 4, canClaimFive: false,
});
assert(decision.stop && decision.cancelTransaction,
    'five-to-four must cancel window while retaining stream ownership');
decision = ownership.decideTouchpadSwipe({
    phase: 'end', fingers: 4, canClaimFive: false,
});
assert(decision.stop && ownership.swipeOwner === StreamOwner.IDLE,
    'owned five-finger END must be stopped, then released');

// Five-finger HOLD/PINCH/SWIPE share one owner. HOLD CANCEL retains ownership
// across the compositor's family transition, while a four-finger BEGIN cannot
// be stolen by a later five-finger UPDATE.
ownership = new InputOwnership();
decision = ownership.decideFiveFinger('hold', 'begin', 5, true);
assert(decision.stop && decision.claimed, 'five-finger HOLD claim failed');
decision = ownership.decideFiveFinger('hold', 'cancel', 5, false);
assert(decision.stop && decision.drainStarted,
    'five-finger HOLD cancel did not enter drain');
decision = ownership.decideFiveFinger('pinch', 'begin', 5, false);
assert(decision.stop, 'five-finger hold-to-pinch transition leaked');
decision = ownership.decideFiveFinger('pinch', 'end', 5, false);
assert(decision.stop && ownership.fiveOwner === StreamOwner.IDLE,
    'five-finger pinch terminal did not release');

ownership = new InputOwnership();
decision = ownership.decideFiveFinger('pinch', 'begin', 4, false);
assert(!decision.stop, 'four-finger pinch BEGIN must remain native');
decision = ownership.decideFiveFinger('pinch', 'update', 5, true);
assert(!decision.stop, 'four-to-five pinch transition stole a native stream');

print('GNOME broker input ownership contract: ok');

function assert(condition, message) {
    if (!condition)
        throw new Error(message);
}
