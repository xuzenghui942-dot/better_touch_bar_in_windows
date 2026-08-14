import Clutter from 'gi://Clutter';
import Meta from 'gi://Meta';
import GLib from 'gi://GLib';

import {
    WindowBackend,
    appSelectionIndex,
    cursorAfterWindowMove,
    minimizeAllWindows,
} from '../windowBackend.js';

class FakeHud {
    constructor() {
        this.badges = 0;
        this.choosers = 0;
        this.strips = 0;
        this.snapPreviews = [];
        this.snapPreviewHides = [];
        this.skippedEffects = [];
    }

    showBadge() {
        this.badges++;
    }

    showChooser() {
        this.choosers++;
    }

    showStrip() {
        this.strips++;
    }

    showSnapPreview(zone, color, monitor) {
        this.snapPreviews.push({zone, color, monitor});
    }

    hideSnapPreview(immediate = false) {
        this.snapPreviewHides.push(immediate);
    }

    skipNextWindowEffect(actor) {
        this.skippedEffects.push(actor);
    }

    hide() {}
}

class FakeAnimationActor {
    constructor(window) {
        this.window = window;
        this.translation_x = 0;
        this.translation_y = 0;
        this.scale_x = 1;
        this.scale_y = 1;
        this.freezeDepth = 0;
        this.easeCalls = 0;
        this.cancelCalls = 0;
        this.lastEase = null;
        this._nextSignalId = 1;
        this._destroySignals = new Map();
        this._transitions = new Map();
    }

    get_transformed_position() {
        return [
            this.window.frame.x + this.translation_x,
            this.window.frame.y + this.translation_y,
        ];
    }
    get_transformed_size() {
        return [
            this.window.frame.width * this.scale_x,
            this.window.frame.height * this.scale_y,
        ];
    }
    freeze() { this.freezeDepth++; }
    thaw() { this.freezeDepth = Math.max(0, this.freezeDepth - 1); }
    connect(signal, callback) {
        assert(signal === 'destroy', 'animation actor must only connect destroy cleanup');
        const id = this._nextSignalId++;
        this._destroySignals.set(id, callback);
        return id;
    }
    disconnect(id) { this._destroySignals.delete(id); }
    remove_all_transitions() {
        this.cancelCalls++;
        for (const [id, params] of this._transitions) {
            GLib.source_remove(id);
            params.onStopped?.(false);
        }
        this._transitions.clear();
    }
    ease(params) {
        this.easeCalls++;
        this.lastEase = params;
        let id = 0;
        id = GLib.timeout_add(GLib.PRIORITY_DEFAULT, params.duration, () => {
            this._transitions.delete(id);
            for (const key of ['translation_x', 'translation_y', 'scale_x', 'scale_y']) {
                if (key in params)
                    this[key] = params[key];
            }
            params.onStopped?.(true);
            return GLib.SOURCE_REMOVE;
        });
        this._transitions.set(id, params);
    }
    destroyDuringAnimation() {
        const callbacks = [...this._destroySignals.values()];
        this._destroySignals.clear();
        for (const callback of callbacks)
            callback();
    }
}

class FakeWindow {
    constructor(flags) {
        this.flags = flags;
        this.restoreFrame = {x: 100, y: 100, width: 900, height: 700};
        this.frame = flags === Meta.MaximizeFlags.BOTH
            ? {x: 0, y: 32, width: 1920, height: 1048}
            : {...this.restoreFrame};
        this.geometryWrites = 0;
        this.monitorMoves = 0;
        this.unmaximizeCalls = 0;
        this.maximizeFlagCalls = 0;
        this.deferVerticalRestore = false;
        this.currentMonitor = 0;
        this.lockGeometryWhileFullyMaximized = false;
        this.valid = true;
        this.actor = new FakeAnimationActor(this);
    }

    get_frame_rect() {
        if (!this.valid)
            throw new Error('window is no longer valid');
        return {...this.frame};
    }
    get_compositor_private() { return this.actor; }
    get_work_area_current_monitor() { return {x: 0, y: 32, width: 1920, height: 1048}; }
    get_maximize_flags() { return this.flags; }
    get_monitor() { return this.currentMonitor; }
    get_work_area_for_monitor(monitor) {
        return monitor === 0
            ? {x: 0, y: 32, width: 1920, height: 1048}
            : {x: 1920, y: 32, width: 1920, height: 1048};
    }
    allows_move() {
        return !this.lockGeometryWhileFullyMaximized ||
            this.flags !== Meta.MaximizeFlags.BOTH;
    }
    allows_resize() {
        return !this.lockGeometryWhileFullyMaximized ||
            this.flags !== Meta.MaximizeFlags.BOTH;
    }
    can_maximize() { return true; }
    can_minimize() { return true; }
    unmake_fullscreen() {}
    unmake_above() {}
    unminimize() {}
    minimize() {}
    unmaximize() {
        this.unmaximizeCalls++;
        if (this.deferVerticalRestore && this.flags === Meta.MaximizeFlags.VERTICAL) {
            GLib.timeout_add(GLib.PRIORITY_DEFAULT, 20, () => {
                this.frame = {...this.restoreFrame};
                return GLib.SOURCE_REMOVE;
            });
        }
        if (this.flags === Meta.MaximizeFlags.BOTH)
            this.frame = {...this.restoreFrame};
        this.flags = 0;
    }
    maximize() {
        if (this.flags !== Meta.MaximizeFlags.BOTH)
            this.restoreFrame = {...this.frame};
        this.flags = Meta.MaximizeFlags.BOTH;
        this.frame = {x: 0, y: 32, width: 1920, height: 1048};
    }
    set_maximize_flags(flags) {
        this.maximizeFlagCalls++;
        this.flags = flags;
    }
    move_to_monitor(monitor) {
        this.currentMonitor = monitor;
        this.monitorMoves++;
    }
    move_frame(_userOp, x, y) {
        this.frame.x = x;
        this.frame.y = y;
        this.geometryWrites++;
    }
    move_resize_frame(_userOp, x, y, width, height) {
        this.frame = {x, y, width, height};
        this.geometryWrites++;
    }
}

function backendFor(flags) {
    const hud = new FakeHud();
    const target = new FakeWindow(flags);
    const backend = new WindowBackend(hud, null);
    backend._target = target;
    backend._settings = {
        gridSpacing: 0,
        halvesEnabled: true,
        quartersEnabled: true,
        maximizeEnabled: true,
        minimizeEnabled: true,
        livePreview: true,
        animateSnaps: false,
        snapAnimationSeconds: 0.08,
        moveCursor: false,
        monitorMoveEnabled: true,
        swipeDownAction: 'minimize',
        swipeDownThreshold: 0.15,
        overlayColor: '#5AC8FA',
    };
    backend._desktopInterfaceSettings = {get_boolean: () => true};
    return {backend, target, hud, skippedEffects: hud.skippedEffects};
}

function assert(condition, message) {
    if (!condition)
        throw new Error(message);
}

for (const startingFlags of [
    0,
    Meta.MaximizeFlags.HORIZONTAL,
    Meta.MaximizeFlags.VERTICAL,
    Meta.MaximizeFlags.BOTH,
]) {
    for (const zone of [
        'leftHalf', 'rightHalf',
        'topLeft', 'topRight', 'bottomLeft', 'bottomRight',
    ]) {
        const {backend, target} = backendFor(startingFlags);
        assert(backend._applyZone(zone), `${zone} must be accepted from flags=${startingFlags}`);
        if (startingFlags === Meta.MaximizeFlags.BOTH)
            runUntil(() => target.geometryWrites === 2);
        const expectedFlags = zone.endsWith('Half')
            ? Meta.MaximizeFlags.VERTICAL
            : 0;
        assert(target.flags === expectedFlags,
            `${zone} must leave exact flags=${expectedFlags}, got ${target.flags}`);
        assert(target.geometryWrites === 2, `${zone} must perform one move+resize transaction`);
        assert(target.monitorMoves === 1, `${zone} must pin to its current monitor`);

        assert(backend._applyZone('maximize'), `${zone}->maximize must be accepted`);
        assert(target.flags === Meta.MaximizeFlags.BOTH,
            `${zone}->maximize must set BOTH, got ${target.flags}`);
    }
}

for (const [source, destination, sourceX, destinationX] of [
    ['leftHalf', 'rightHalf', 0, 960],
    ['rightHalf', 'leftHalf', 960, 0],
]) {
    const {backend, target} = backendFor(Meta.MaximizeFlags.VERTICAL);
    target.frame = {x: sourceX, y: 32, width: 960, height: 1048};
    target.restoreFrame = {x: 140, y: 100, width: 1180, height: 760};
    target.deferVerticalRestore = true;
    backend._settings.animateSnaps = true;

    assert(backend._applyZone(destination),
        `${source}->${destination} must be accepted`);
    runMainContextFor(300);
    assert(target.frame.x === destinationX && target.frame.y === 32 &&
        target.frame.width === 960 && target.frame.height === 1048,
    `${source}->${destination} must not be pulled back by a late Wayland restore`);
    assert(target.unmaximizeCalls === 0 && target.maximizeFlagCalls === 0,
        `${source}->${destination} must preserve the existing VERTICAL state`);
    assert(target.geometryWrites === 2 && target.monitorMoves === 1,
        `${source}->${destination} must keep one move+resize transaction`);
    assert(target.actor.easeCalls === 1 &&
        target.actor.lastEase.mode === Clutter.AnimationMode.EASE_OUT_QUART &&
        target.actor.lastEase.duration >= 180 && target.actor.lastEase.duration <= 260,
    `${source}->${destination} must keep the adaptive no-bounce FLIP`);
}

{
    const {backend, target} = backendFor(Meta.MaximizeFlags.VERTICAL);
    target.frame = {x: 0, y: 32, width: 960, height: 1048};
    assert(backend._applyZone('topRight'),
        'half-screen to quarter-screen must remain accepted');
    assert(target.unmaximizeCalls === 1 && target.maximizeFlagCalls === 0,
        'quarter snaps must retain the v5 maximize-state transition');
}

{
    const {backend, target} = backendFor(Meta.MaximizeFlags.BOTH);
    target.lockGeometryWhileFullyMaximized = true;
    assert(backend._applyZone('rightHalf'),
        'a fully maximized window must be restored before checking move/resize permission');
    runUntil(() => target.geometryWrites === 2);
    assert(target.frame.x === 960 && target.frame.width === 960,
        'a restored maximized window must commit the requested right-half geometry');
    assert(target.unmaximizeCalls === 1 && target.maximizeFlagCalls === 1,
        'fully-maximized to half-screen must retain the v5 state transition');
}

{
    const {backend, target} = backendFor(0);
    target.frame = {x: 0, y: 32, width: 960, height: 1048};
    target.flags = Meta.MaximizeFlags.VERTICAL;
    backend._settings.animateSnaps = true;
    assert(backend._applyZone('maximize'),
        'maximize animation must enter its asynchronous settle window');
    assert(backend._committedSources.size === 1,
        'maximize animation must track its settle source');
    target.valid = false;
    target.actor.destroyDuringAnimation();
    assert(backend._committedAnimations.size === 0 &&
        backend._committedSources.size === 0 && target.actor.freezeDepth === 0,
    'actor destruction must synchronously clear settle sources and visual state');
}

{
    const {backend, target, skippedEffects} = backendFor(0);
    target.frame = {x: 960, y: 32, width: 960, height: 1048};
    target.flags = Meta.MaximizeFlags.VERTICAL;
    backend._settings.animateSnaps = true;
    backend._settings.snapAnimationSeconds = 0.20;
    assert(backend._applyZone('maximize'),
        'animated half-screen to maximize transition must be accepted');
    assert(target.flags === Meta.MaximizeFlags.BOTH && target.frame.x === 0 &&
        target.frame.width === 1920,
    'maximize must submit its final state immediately');
    assert(skippedEffects.length === 1,
        'custom maximize animation must feature-detect and suppress the duplicate Shell effect');
    runUntil(() => target.actor.easeCalls === 1);
    assert(target.actor.lastEase.duration === 200 &&
        target.actor.lastEase.mode === Clutter.AnimationMode.EASE_OUT_CUBIC,
    'maximize must preserve its configured duration and existing cubic curve');
    backend.finish();
    runMainContextFor(250);
    assert(backend._committedAnimations.size === 0 &&
        backend._committedSources.size === 0 && target.actor.freezeDepth === 0,
    'maximize animation must survive gesture finish and then clean up safely');
}

{
    const {backend, target, skippedEffects} = backendFor(Meta.MaximizeFlags.BOTH);
    backend._originalMaximizeFlags = Meta.MaximizeFlags.BOTH;
    backend._settings.animateSnaps = false;
    assert(backend._applyZone('maximize'),
        'animation-disabled maximize gesture must still restore a fully maximized window');
    assert(target.flags === 0 && target.frame.x === 100 && target.frame.width === 900,
        'animation-disabled restore must commit its final state immediately');
    assert(skippedEffects.length === 1 && backend._committedAnimations.size === 0,
        'animation-disabled state change must skip Shell animation and create no actor transition');
}

{
    const {backend, target, hud} = backendFor(Meta.MaximizeFlags.BOTH);
    backend._mode = 'swipe';
    backend._settings.livePreview = false;
    assert(backend.handleUpdate({kind: 'updated', direction: 'left', progress: 1}),
        'direction update must accept the zone without drawing a large preview');
    runMainContextFor(30);
    assert(hud.badges === 0,
        'normal direction updates must not fall back to the text badge');
    assert(hud.snapPreviews.length === 1 && hud.snapPreviews[0].zone === 'leftHalf',
        'normal direction updates must show a compact pointer-adjacent snap map');
    assert(target.geometryWrites === 0,
        'non-live preview must not mutate the real window before release');
    assert(target.flags === Meta.MaximizeFlags.BOTH,
        'non-live preview must preserve maximize state');
    assert(backend.handleCommit({kind: 'completed', direction: 'left'}),
        'release must still commit the selected non-live zone');
    assert(hud.snapPreviewHides.at(-1) === true,
        'releasing a snap must hide its preview immediately before window motion');
    runUntil(() => target.geometryWrites === 2);
    assert(target.frame.x === 0 && target.frame.width === 960,
        'non-live release must preserve the exact left-half destination');
}

{
    const {backend, target, hud} = backendFor(0);
    backend._mode = 'swipe';
    backend._settings.livePreview = false;
    assert(backend.handleUpdate({kind: 'pinchUpdated', outward: true}),
        'pinch preview must remain available');
    assert(hud.badges === 1,
        'special-mode feedback must use a compact badge instead of a large zone actor');
    assert(target.geometryWrites === 0,
        'pinch feedback must not mutate geometry before release');
}

{
    const {backend, target} = backendFor(0);
    backend._settings.animateSnaps = false;
    assert(backend._applyZone('bottomRight'),
        'animation-disabled corner snap must be accepted');
    assert(target.geometryWrites === 2,
        'animation-disabled snap must commit one immediate move+resize transaction');
    assert(target.frame.x === 960 && target.frame.y === 556 &&
        target.frame.width === 960 && target.frame.height === 524,
    'animation-disabled snap must land on the exact final rectangle immediately');
}

{
    const {backend, target} = backendFor(0);
    target.frame = {x: 120, y: 100, width: 900, height: 700};
    backend._settings.animateSnaps = true;
    backend._neighborMonitor = () => 1;
    assert(backend._moveToMonitor('right'),
        'animated cross-monitor move must be accepted');
    assert(target.currentMonitor === 1 && target.frame.x === 2040 &&
        target.geometryWrites === 1,
    'cross-monitor animation must submit the final monitor and position once');
    runMainContextFor(150);
    assert(backend._committedAnimations.size === 0 && target.actor.freezeDepth === 0,
        'cross-monitor compositor transition must clean up');
}

{
    const {backend, target, skippedEffects} = backendFor(0);
    backend._settings.animateSnaps = true;
    assert(backend._applyZone('rightHalf'), 'animated snap must be accepted');
    assert(target.frame.x === 960 && target.frame.width === 960,
        'animated snap must submit the exact final geometry immediately');
    assert(target.actor.easeCalls === 1 && skippedEffects.length === 0,
        'animated snap must use one actor transition without leaving a stale Shell skip');
    assert(target.actor.lastEase.duration >= 180 && target.actor.lastEase.duration <= 260 &&
        target.actor.lastEase.mode === Clutter.AnimationMode.EASE_OUT_QUART,
    'half/quarter snaps must use the 180-260ms no-bounce adaptive curve');
    assert(target.actor.translation_x === -860 && target.actor.translation_y === 68 &&
        Math.abs(target.actor.scale_x - 0.9375) < 0.0001 &&
        Math.abs(target.actor.scale_y - (700 / 1048)) < 0.0001,
    'single-actor FLIP must begin at the exact pre-commit visual rectangle');
    runMainContextFor(300);
    assert(target.geometryWrites === 2,
        'compositor animation must not stream per-frame Meta geometry writes');
    assert(target.frame.x === 960 && target.frame.width === 960,
        'animated snap final geometry must remain exact');
    assert(backend._committedAnimations.size === 0 &&
        target.actor.freezeDepth === 0 && target.actor.scale_x === 1 &&
        target.actor.scale_y === 1 && target.actor.translation_x === 0 &&
        target.actor.translation_y === 0,
    'completed compositor animation must restore every actor property');
}

{
    const {backend, hud} = backendFor(0);
    hud.hide = () => {
        throw new Error('HUD actor was disposed by GNOME');
    };
    let threw = false;
    try {
        backend.finish();
    } catch (_error) {
        threw = true;
    }
    assert(!threw, 'HUD disposal must not escape the window transaction cleanup');
    assert(!backend.hasTarget(), 'HUD disposal must not retain the active window target');
}

{
    const {backend, target} = backendFor(0);
    backend._settings.animateSnaps = true;
    assert(backend._applyZone('leftHalf'), 'first animated snap must be accepted');
    backend._cancelCommittedForTarget(target, {preserveRunningAdaptive: true});
    assert(backend._committedAnimations.has(target),
        'beginning a new gesture must not reset a running half/quarter FLIP');
    target.actor.translation_x = 137;
    target.actor.translation_y = 19;
    target.actor.scale_x = 0.83;
    target.actor.scale_y = 0.79;
    const presentation = {
        x: target.frame.x + target.actor.translation_x,
        y: target.frame.y + target.actor.translation_y,
        width: target.frame.width * target.actor.scale_x,
        height: target.frame.height * target.actor.scale_y,
    };
    assert(backend._applyZone('rightHalf'), 'replacement animated snap must be accepted');
    assert(Math.abs(target.frame.x + target.actor.translation_x - presentation.x) < 0.001 &&
        Math.abs(target.frame.y + target.actor.translation_y - presentation.y) < 0.001 &&
        Math.abs(target.frame.width * target.actor.scale_x - presentation.width) < 0.001 &&
        Math.abs(target.frame.height * target.actor.scale_y - presentation.height) < 0.001,
    'replacement FLIP must start from the live presentation rectangle without an identity jump');
    runMainContextFor(300);
    assert(target.frame.x === 960 && target.frame.width === 960,
        'an older animation must never overwrite a newer destination');
    assert(target.actor.cancelCalls >= 2,
        'starting a second animation must cancel the first actor transition');
    assert(backend._committedSources.size === 0,
        'replacement animations must not leak GLib sources');
}

{
    const {backend, target} = backendFor(0);
    backend._settings.animateSnaps = true;
    backend._desktopInterfaceSettings = {get_boolean: () => false};
    assert(backend._applyZone('topRight'),
        'system-reduced-motion corner snap must still be accepted');
    assert(target.actor.easeCalls === 0 && backend._committedAnimations.size === 0,
        'GNOME system animations disabled must make half/quarter snaps immediate');
    assert(target.frame.x === 960 && target.frame.y === 32 &&
        target.frame.width === 960 && target.frame.height === 524,
    'reduced-motion snap must still submit the exact final geometry');
}

{
    const {backend, target} = backendFor(0);
    backend._settings.animateSnaps = true;
    assert(backend._applyZone('leftHalf'), 'animated snap must start before invalidation');
    target.valid = false;
    target.actor.destroyDuringAnimation();
    runMainContextFor(120);
    assert(backend._committedAnimations.size === 0 &&
        backend._committedSources.size === 0,
    'target invalidation must clear committed animation state and sources');
    assert(target.actor.freezeDepth === 0,
        'target invalidation must thaw the actor and reset its visual transform');
}

{
    const {backend, target} = backendFor(Meta.MaximizeFlags.BOTH);
    target.lockGeometryWhileFullyMaximized = true;
    backend._settings.animateSnaps = true;
    assert(backend._applyZone('rightHalf'),
        'animated fully-maximized right-half transition must be accepted');
    runUntil(() => target.geometryWrites === 2);
    assert(target.frame.x === 960 && target.frame.width === 960 &&
        target.flags === Meta.MaximizeFlags.VERTICAL,
    'animated fully-maximized transition must preserve the restored-before-check fix');
    runMainContextFor(300);
    assert(backend._committedAnimations.size === 0 && target.actor.freezeDepth === 0,
        'animated fully-maximized transition must clean up compositor state');
}

{
    const {backend, target, hud} = backendFor(0);
    backend._mode = 'swipe';
    assert(backend.handleUpdate({kind: 'updated', direction: 'left', progress: 1}),
        'legacy live-preview setting must still accept a half-screen update');
    runMainContextFor(30);
    assert(target.geometryWrites === 0 && hud.snapPreviews.length === 1,
        'half/quarter prediction must never move the real window before release');
}

{
    const point = cursorAfterWindowMove(
        {x: 340, y: 180},
        {x: 100, y: 100, width: 900, height: 700},
        {x: 0, y: 32, width: 960, height: 1048});
    assert(point.x === 240 && point.y === 112,
        'cursor follow must preserve the cursor offset inside the window');
    assert(appSelectionIndex(2, -9, 5) === 0 && appSelectionIndex(2, 9, 5) === 4,
        'hold app selection must clamp to visible window bounds');
}

{
    const windows = [
        fakeMinimizableWindow(),
        fakeMinimizableWindow(),
        fakeMinimizableWindow({skipTaskbar: true}),
        fakeMinimizableWindow({type: Meta.WindowType.DIALOG}),
        fakeMinimizableWindow({canMinimize: false}),
        fakeMinimizableWindow({minimized: true}),
    ];
    assert(minimizeAllWindows(windows) === 2,
        'four-finger down must minimize every eligible normal window exactly once');
    assert(windows[0].minimizeCalls === 1 && windows[1].minimizeCalls === 1,
        'eligible active-workspace windows were not minimized');
    assert(windows.slice(2).every(window => window.minimizeCalls === 0),
        'ineligible windows must remain unchanged');
}

{
    const {backend} = backendFor(0);
    backend._mode = 'swipe';
    backend._settings.livePreview = false;
    backend._settings.swipeDownAction = 'choose';
    backend._priorDirection = 'left';
    backend._priorDirectionSince = GLib.get_monotonic_time() - 200_000;
    backend._lastDy = 0.20;
    backend._previewSwipe({direction: 'down', progress: 1});
    assert(backend._downLatched, 'down chooser must latch after crossing its threshold');
    backend._lastDy = 0.10;
    backend._previewSwipe({direction: 'down', progress: 1});
    assert(!backend._downLatched,
        'retracting the chooser must release its down-action latch');
    assert(backend.takeRebaselineRequest() === 'left',
        'chooser reversal must ask Rust to reseed the prior snap direction');
    backend._previewSwipe({direction: 'down', progress: 1});
    assert(!backend._downLatched,
        'stale down frames must not relatch before the reseed reaches Rust');
    backend._previewSwipe({direction: 'left', progress: 1});
    assert(!backend._awaitingRebaseline,
        'the restored direction must acknowledge the asynchronous reseed');
}

print('GNOME window state transition contract: ok');

function runUntil(predicate) {
    const loop = GLib.MainLoop.new(null, false);
    const timeoutId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 500, () => {
        loop.quit();
        return GLib.SOURCE_REMOVE;
    });
    const pollId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 5, () => {
        if (!predicate())
            return GLib.SOURCE_CONTINUE;
        loop.quit();
        return GLib.SOURCE_REMOVE;
    });
    loop.run();
    if (GLib.MainContext.default().find_source_by_id(timeoutId))
        GLib.source_remove(timeoutId);
    if (GLib.MainContext.default().find_source_by_id(pollId))
        GLib.source_remove(pollId);
    assert(predicate(), 'timed out waiting for deferred maximized transition');
}

function runMainContextFor(durationMs) {
    const loop = GLib.MainLoop.new(null, false);
    GLib.timeout_add(GLib.PRIORITY_DEFAULT, durationMs, () => {
        loop.quit();
        return GLib.SOURCE_REMOVE;
    });
    loop.run();
}

function fakeMinimizableWindow(options = {}) {
    return {
        minimized: options.minimized === true,
        minimizeCalls: 0,
        is_skip_taskbar() { return options.skipTaskbar === true; },
        get_window_type() { return options.type ?? Meta.WindowType.NORMAL; },
        can_minimize() { return options.canMinimize !== false; },
        minimize() { this.minimized = true; this.minimizeCalls++; },
    };
}
