import Clutter from 'gi://Clutter';
import GObject from 'gi://GObject';
import Meta from 'gi://Meta';

const requiredWindowMethods = [
    'allows_move',
    'allows_resize',
    'can_maximize',
    'can_minimize',
    'can_close',
    'change_workspace',
    'delete',
    'get_frame_rect',
    'get_compositor_private',
    'get_gtk_application_id',
    'get_sandboxed_app_id',
    'get_title',
    'get_wm_class',
    'get_wm_class_instance',
    'get_work_area_current_monitor',
    'get_work_area_for_monitor',
    'maximize',
    'minimize',
    'move_frame',
    'move_resize_frame',
    'move_to_monitor',
    'set_maximize_flags',
    'unmaximize',
];

for (const method of requiredWindowMethods) {
    if (typeof Meta.Window.prototype[method] !== 'function')
        throw new Error(`GNOME 50 is missing Meta.Window.${method}`);
}

for (const method of ['get_transformed_position', 'get_transformed_size']) {
    if (typeof Clutter.Actor.prototype[method] !== 'function')
        throw new Error(`GNOME 50 is missing Clutter.Actor.${method}`);
}

for (const method of ['freeze', 'thaw']) {
    if (typeof Meta.WindowActor.prototype[method] !== 'function')
        throw new Error(`GNOME 50 is missing Meta.WindowActor.${method}`);
}

if (typeof Clutter.Seat.prototype.warp_pointer !== 'function')
    throw new Error('GNOME 50 is missing Clutter.Seat.warp_pointer');
for (const method of ['remove_all_transitions']) {
    if (typeof Clutter.Actor.prototype[method] !== 'function')
        throw new Error(`GNOME 50 is missing Clutter.Actor.${method}`);
}

if (typeof Meta.WorkspaceManager.prototype.append_new_workspace !== 'function')
    throw new Error('GNOME 50 is missing Meta.WorkspaceManager.append_new_workspace');

for (const detailedSignal of [
    'captured-event::key',
    'captured-event::touchpad',
]) {
    const [valid] = GObject.signal_parse_name(
        detailedSignal, Clutter.Stage.$gtype, true);
    if (!valid)
        throw new Error(`GNOME 50 is missing ${detailedSignal}`);
}

for (const eventType of [
    'TOUCHPAD_SWIPE',
    'TOUCHPAD_PINCH',
    'TOUCHPAD_HOLD',
]) {
    if (typeof Clutter.EventType[eventType] !== 'number')
        throw new Error(`GNOME 50 is missing Clutter.EventType.${eventType}`);
}

for (const method of [
    'get_gesture_phase',
    'get_gesture_motion_delta_unaccelerated',
    'get_touchpad_gesture_finger_count',
]) {
    if (typeof Clutter.Event.prototype[method] !== 'function')
        throw new Error(`GNOME 50 is missing Clutter.Event.${method}`);
}

print(`GNOME 50 API contract: ${requiredWindowMethods.length} window methods and ` +
    'Clutter detailed signals ok');
