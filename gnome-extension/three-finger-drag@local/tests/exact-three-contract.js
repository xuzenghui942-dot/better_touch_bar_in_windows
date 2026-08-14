import {ExactThreeGuard, ExactThreeGuardSet} from '../exactThreeGuard.js';

const guard = new ExactThreeGuard();
assert(guard.decide('begin', 3), 'exact-three BEGIN must stop without a broker');
assert(guard.decide('update', 4),
    'a stopped three-finger stream must not leak after its finger count changes');
assert(guard.decide('end', 4), 'the owned terminal event must be stopped');
assert(!guard.owned && !guard.decide('begin', 4),
    'an independent four-finger stream must propagate');
assert(!guard.decide('update', 3),
    'a GNOME-owned four-finger stream must not be stolen after dropping to three');
assert(!guard.decide('end', 3),
    'the terminal event of a GNOME-owned stream must propagate');

const guards = new ExactThreeGuardSet();
const internal = {name: 'internal'};
const external = {name: 'external'};
assert(guards.decide(internal, 'swipe', 'begin', 3),
    'internal touchpad exact-three BEGIN must stop');
assert(!guards.decide(external, 'swipe', 'begin', 4),
    'an external touchpad four-finger BEGIN must remain independent');
assert(!guards.decide(external, 'swipe', 'end', 4),
    'external terminal must not alter internal ownership');
assert(guards.decide(internal, 'swipe', 'update', 3),
    'external terminal cleared another device ownership');
assert(guards.decide(internal, 'swipe', 'end', 3),
    'internal terminal must be stopped before its state is deleted');
assert(!guards.decide(null, 'swipe', 'begin', 3),
    'events without a stable source device must fail open');

assert(guards.decide(internal, 'hold', 'begin', 3),
    'three-finger hold BEGIN must stop');
assert(!guards.decide(internal, 'pinch', 'begin', 4),
    'a separate four-finger pinch must not corrupt hold ownership');
assert(guards.decide(internal, 'hold', 'cancel', 3),
    'hold terminal must retain its own family ownership');
assert(!guards.decide(internal, 'pinch', 'end', 4),
    'pinch terminal must remain independent');

print('GNOME exact-three fallback contract: ok');

function assert(condition, message) {
    if (!condition)
        throw new Error(message);
}
