import {FourFingerDownGesture, FourFingerDownGestureSet} from '../fourFingerDown.js';

let gesture = new FourFingerDownGesture();
assert(!gesture.observe('begin', 4, 0, 0), 'BEGIN must not commit');
assert(!gesture.observe('update', 4, 5, 40), 'UPDATE must not commit early');
assert(gesture.observe('end', 4, 4, 40), 'complete downward swipe must commit');

gesture = new FourFingerDownGesture();
gesture.observe('begin', 4, 0, 0);
gesture.observe('update', 4, 90, 75);
assert(!gesture.observe('end', 4, 0, 0), 'diagonal swipe must remain GNOME-owned');

gesture = new FourFingerDownGesture();
gesture.observe('begin', 4, 0, 0);
gesture.observe('update', 4, 0, -90);
assert(!gesture.observe('end', 4, 0, 0), 'upward swipe must not commit');

gesture = new FourFingerDownGesture();
gesture.observe('begin', 4, 0, 0);
gesture.observe('update', 3, 0, 80);
assert(!gesture.observe('end', 4, 0, 20), 'finger-count change must cancel');

gesture = new FourFingerDownGesture();
gesture.observe('begin', 4, 0, 0);
gesture.observe('update', 4, 0, 90);
assert(!gesture.observe('cancel', 4, 0, 0), 'CANCEL must not commit');

const set = new FourFingerDownGestureSet();
const internal = {name: 'internal'};
const external = {name: 'external'};
set.observe(internal, 'begin', 4, 0, 0);
set.observe(external, 'begin', 4, 0, 0);
set.observe(internal, 'update', 4, 0, 80);
assert(set.observe(internal, 'end', 4, 0, 0), 'device streams must be independent');
assert(!set.observe(external, 'end', 4, 0, 0), 'idle second device must not commit');
assert(!set.observe(null, 'begin', 4, 0, 100), 'unknown devices must fail open');

print('GNOME passive four-finger-down contract: ok');

function assert(condition, message) {
    if (!condition)
        throw new Error(message);
}
