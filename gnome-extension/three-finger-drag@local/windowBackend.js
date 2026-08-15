import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';

import {
    adaptiveSnapDuration,
    clampRect,
    copyRect,
    isHalfOrQuarterZone,
    standardZone,
    zoneRect,
} from './geometry.js';

const FRAME_INTERVAL_MS = 16;
const MAXIMIZED_SETTLE_INTERVAL_MS = 20;
const MAXIMIZED_SETTLE_ATTEMPTS = 6;
const TITLEBAR_HEIGHT = 44;

export class WindowBackend {
    constructor(hud, onTargetGone) {
        this._hud = hud;
        this._onTargetGone = onTargetGone;
        this._committedSources = new Set();
        this._sourceTargets = new Map();
        this._committedAnimations = new Map();
        try {
            this._desktopInterfaceSettings = new Gio.Settings({
                schema_id: 'org.gnome.desktop.interface',
            });
        } catch (_error) {
            this._desktopInterfaceSettings = null;
        }
        this._resetState();
    }

    configure(settings) {
        this._configuredSettings = settings;
        this._hud.configure?.(settings);
    }

    probeTarget(settings) {
        if (this._target)
            return {accepted: false, target: null, detail: 'another target is active'};
        const target = this._targetUnderPointer(settings);
        return target
            ? {accepted: true, target, detail: 'manageable titlebar target probed'}
            : {accepted: false, target: null, detail: this._lastTargetDetail};
    }

    begin(settings, event, probedTarget) {
        if (this._target)
            return {accepted: false, detail: 'another target is active'};
        if (event.kind === 'began' && event.contacts !== 2)
            return {accepted: false, detail: 'only the two-finger advanced gesture is supported'};
        if (event.kind === 'freeMoveBegan' && !settings.fiveFingerEnabled)
            return {accepted: false, detail: 'five-finger free move is disabled'};

        const target = this._targetUnderPointer(settings);
        if (!target)
            return {accepted: false, detail: this._lastTargetDetail};
        if (target !== probedTarget)
            return {accepted: false, detail: 'probed titlebar target changed before Begin'};

        // A finished half/quarter transaction may still be visually settling.
        // Let that compositor-only transition continue until a new snap is
        // actually committed, then retarget from its live presentation value.
        // Deferred state changes and every non-snap transition still cancel
        // immediately so they cannot overwrite the new gesture.
        this._cancelCommittedForTarget(target, {preserveRunningAdaptive: true});

        this._settings = settings;
        this._lastSettings = settings;
        this._target = target;
        this._mode = event.kind === 'freeMoveBegan' ? 'free' : 'swipe';
        this._original = copyRect(target.get_frame_rect());
        this._originalMaximizeFlags = target.get_maximize_flags();
        this._originalWorkspace = target.get_workspace();
        this._originalWorkspaceIndex = this._originalWorkspace.index();
        this._originalMonitor = target.get_monitor();
        this._currentRect = copyRect(this._original);
        this._lastDx = 0;
        this._lastDy = 0;
        this._mutated = false;
        this._targetUnmanagingId = target.connect('unmanaging', () => {
            this.abandonTarget();
            this._onTargetGone?.();
        });

        if (this._mode === 'free' && this._originalMaximizeFlags !== 0) {
            target.unmaximize();
            // Unmaximizing is already a visible mutation.  A prelock timeout,
            // Escape, rejected Begin, or cancel before the first delta must
            // still restore the original maximize flags.
            this._mutated = true;
        }
        // Do not show an empty HUD as soon as two fingers touch. Normal snap
        // direction updates remain visual-free unless live preview is enabled.
        return {accepted: true, detail: 'target locked by the extension'};
    }

    handleUpdate(event) {
        if (!this._targetAlive())
            return false;

        switch (event.kind) {
        case 'raw':
            if (this._mode !== 'swipe')
                return false;
            this._lastDx = event.dx;
            this._lastDy = event.dy;
            this._downMaxY = Math.max(this._downMaxY, event.dy);
            return true;
        case 'updated':
            if (this._mode !== 'swipe')
                return false;
            this._queueFrame({kind: 'preview', event});
            return true;
        case 'freeMoveDelta':
            if (this._mode !== 'free')
                return false;
            this._queueFreeDelta(event);
            return true;
        case 'axisResizeBegan':
            if (this._mode !== 'swipe' || !this._axisAllowed(event.horizontal) ||
                !this._target.allows_resize())
                return false;
            this._enterMode('axis');
            this._axisHorizontal = event.horizontal;
            if (this._originalMaximizeFlags !== 0) {
                this._target.unmaximize();
                this._mutated = true;
            }
            return true;
        case 'axisResizeDelta':
            if (this._mode !== 'axis' || event.horizontal !== this._axisHorizontal)
                return false;
            this._queueAxisFactor(event.factor);
            return true;
        case 'pinchUpdated': {
            if (!['swipe', 'pinch'].includes(this._mode))
                return false;
            this._enterMode('pinch');
            const zone = event.outward ? 'maximize' : 'center';
            this._hud.showBadge?.(
                `Pinch ${event.outward ? 'maximize' : 'restore'}`,
                this._settings.overlayColor);
            return true;
        }
        case 'monitorMoveUpdated':
            if (!['swipe', 'monitor'].includes(this._mode))
                return false;
            this._enterMode('monitor');
            this._showMonitorPreview(event.direction);
            return true;
        case 'holdEngaged':
            if (!['swipe', 'hold'].includes(this._mode))
                return false;
            this._enterMode('hold');
            this._beginHoldMode();
            return true;
        case 'holdUpdated':
            if (this._mode !== 'hold')
                return false;
            this._holdAimSteps = event.aimSteps;
            this._updateHoldPreview();
            return true;
        case 'desktopMove':
            return this._mode === 'hold' && !this._appSwitchActive &&
                this._moveToWorkspace(event.direction, 1, true);
        default:
            return false;
        }
    }

    handleCommit(event) {
        if (!this._targetAlive())
            return false;
        this._flushPending();

        switch (event.kind) {
        case 'completed': {
            if (this._mode !== 'swipe')
                return false;
            if (this._downLatched)
                return this._downEngaged ? this._applyDownAction() : true;
            const zone = this._currentZone(event.direction);
            if (zone === 'none')
                return this._restorePreviewAndAccept();
            if (zone === 'minimize') {
                if (this._downMaxY < this._settings.swipeDownThreshold)
                    return this._restorePreviewAndAccept();
                return this._applyDownAction();
            }
            if (this._settings.livePreview && !isHalfOrQuarterZone(zone) &&
                this._livePreviewZone === zone)
                return true;
            return this._applyZone(zone, true);
        }
        case 'freeMoveEnded':
            if (this._mode !== 'free' || event.cancelled)
                return false;
            if (event.wasTap && this._settings.centerEnabled)
                return this._applyZone('center');
            return true;
        case 'axisResizeEnded':
            return this._mode === 'axis' && !event.cancelled;
        case 'pinchOut':
            if (this._mode !== 'pinch' || !this._settings.maximizeEnabled ||
                !this._target.can_maximize())
                return false;
            if (this._target.get_maximize_flags() === 0) {
                this._target.maximize();
                this._mutated = true;
            }
            return true;
        case 'pinchIn':
            if (this._mode !== 'pinch')
                return false;
            if (this._target.get_maximize_flags() !== 0) {
                this._target.unmaximize();
                this._mutated = true;
            }
            return true;
        case 'monitorMove':
            return this._mode === 'monitor' && this._moveToMonitor(event.direction);
        case 'desktopHoldCommit':
            if (this._mode !== 'hold')
                return false;
            if (this._appSwitchActive)
                return this._commitAppSwitch(event.steps);
            return event.steps === 0 || this._moveToWorkspace(
                event.steps < 0 ? 'left' : 'right', Math.abs(event.steps), false);
        default:
            return false;
        }
    }

    cancel() {
        this._discardPending();
        if (this._targetAlive() && this._mutated)
            this._restoreOriginal();
        this.finish();
    }

    finish() {
        this._discardPending();
        this._disconnectTarget();
        try {
            this._hideSnapPreview(true);
            this._hideHud();
        } finally {
            this._resetState();
        }
    }

    abandonTarget() {
        this._discardPending();
        if (this._target)
            this._cancelCommittedForTarget(this._target);
        this._targetUnmanagingId = 0;
        try {
            this._hideSnapPreview(true);
            this._hideHud();
        } finally {
            this._resetState();
        }
    }

    destroy() {
        for (const target of [...this._committedAnimations.keys()])
            this._cancelCommittedForTarget(target);
        for (const sourceId of this._committedSources)
            GLib.source_remove(sourceId);
        this._committedSources.clear();
        this._sourceTargets.clear();
        this.cancel();
    }

    hasTarget() {
        return this._target !== null;
    }

    hasMutated() {
        return this._mutated;
    }

    takeRebaselineRequest() {
        const direction = this._pendingRebaseline ?? '';
        this._pendingRebaseline = null;
        return direction;
    }

    getModifiers() {
        const settings = this._settings ?? this._configuredSettings ?? {
            monitorMoveEnabled: true,
            monitorMoveModifier: 'alt',
        };
        return {
            monitor: settings.monitorMoveEnabled === true &&
                modifierPressed(settings.monitorMoveModifier),
        };
    }

    minimizeAllOnActiveWorkspace() {
        const workspace = global.workspace_manager.get_active_workspace();
        const windows = global.display.sort_windows_by_stacking(
            workspace.list_windows()).reverse();
        return minimizeAllWindows(windows);
    }

    _targetUnderPointer(settings) {
        const [pointerX, pointerY] = global.get_pointer();
        const workspace = global.workspace_manager.get_active_workspace();
        const stacked = global.display
            .sort_windows_by_stacking(workspace.list_windows())
            .reverse();
        const focused = global.display.get_focus_window();
        const windows = focused === null
            ? stacked
            : [focused, ...stacked.filter(window => window !== focused)];
        const rejected = [];
        for (const window of windows) {
            const reason = this._unmanageableReason(window);
            if (reason !== null) {
                if (window === focused)
                    rejected.push(`focus=${reason}`);
                continue;
            }
            if (this._compatibilityBlocks(window, settings)) {
                if (window === focused)
                    rejected.push('focus=blocked by application rule');
                continue;
            }
            if (this._pointerInTitlebar(window, pointerX, pointerY))
                return window;
            if (window === focused) {
                const rect = window.get_frame_rect();
                rejected.push(`focusFrame=${formatRect(rect)}`);
            }
        }
        this._lastTargetDetail = `pointer (${Math.round(pointerX)},${Math.round(pointerY)}) ` +
            `is not in a manageable titlebar${rejected.length ? `; ${rejected.join('; ')}` : ''}`;
        return null;
    }

    _manageable(window) {
        return this._unmanageableReason(window) === null;
    }

    _unmanageableReason(window) {
        try {
            if (window.minimized)
                return 'minimized';
            if (window.is_fullscreen())
                return 'fullscreen';
            if (window.is_skip_taskbar())
                return 'skip-taskbar';
            if (window.is_override_redirect())
                return 'override-redirect';
            if (window.is_attached_dialog())
                return 'attached-dialog';
            if (window.get_window_type() !== Meta.WindowType.NORMAL)
                return `window-type-${window.get_window_type()}`;
            if (!(window.allows_move() || window.allows_resize() ||
                window.can_maximize() || window.can_minimize()))
                return 'no supported window action';
            return null;
        } catch (error) {
            return `GNOME API error: ${error.message}`;
        }
    }

    _pointerInTitlebar(window, pointerX, pointerY) {
        const rect = window.get_frame_rect();
        const scale = Math.max(1, global.display.get_monitor_scale(window.get_monitor()));
        // Meta frame coordinates and Shell stage coordinates are normally the
        // same logical space. Keep the band bounded for CSD clients and
        // fractional scaling rather than treating the entire client surface
        // as a titlebar.
        const frameBand = Math.min(rect.height, Math.max(
            TITLEBAR_HEIGHT,
            Math.min(96, TITLEBAR_HEIGHT * scale)));
        if (pointInTopBand(pointerX, pointerY, rect, frameBand))
            return true;

        // GNOME 50 CSD/XWayland actors can have transformed stage extents that
        // differ from Meta's frame rectangle. This is still a Shell-selected
        // target; Rust never supplies a window id or arbitrary coordinates.
        try {
            const actor = window.get_compositor_private();
            if (actor) {
                const [x, y] = actor.get_transformed_position();
                const [width, height] = actor.get_transformed_size();
                const actorBand = Math.min(height, Math.max(
                    TITLEBAR_HEIGHT,
                    Math.min(96, height * 0.18)));
                if (pointInTopBand(pointerX, pointerY, {x, y, width, height}, actorBand))
                    return true;
            }
        } catch (_error) {
            // Fall back to the Meta frame geometry above.
        }
        return false;
    }

    _compatibilityBlocks(window, settings) {
        if (settings.appCompatibilityProcessNames.length === 0)
            return false;
        const configured = new Set(settings.appCompatibilityProcessNames
            .flatMap(identifierVariants));
        const candidates = [
            window.get_gtk_application_id(),
            window.get_sandboxed_app_id(),
            window.get_wm_class(),
            window.get_wm_class_instance(),
        ].filter(value => typeof value === 'string').flatMap(identifierVariants);
        if (!candidates.some(candidate => configured.has(candidate)))
            return false;
        if (settings.appCompatibilityMode === 'exclude')
            return true;
        return !modifierPressed(settings.appCompatibilityModifier);
    }

    _currentZone(direction) {
        return standardZone(direction, this._settings);
    }

    _queueFrame(pending) {
        this._pending = pending;
        this._ensureFrameSource();
    }

    _enterMode(mode) {
        if (this._mode === mode)
            return;
        this._discardPending();
        // A live snap preview belongs to the swipe family. Restore it before
        // switching to pinch/axis/hold/monitor so a later terminal event
        // cannot accidentally commit stale preview geometry.
        if (this._mutated)
            this._restoreOriginal();
        this._hideSnapPreview(true);
        this._mode = mode;
    }

    _queueFreeDelta(event) {
        if (this._pending?.kind === 'free') {
            this._pending.dx += event.dx;
            this._pending.dy += event.dy;
            this._pending.scale *= event.scale;
        } else {
            this._pending = {kind: 'free', dx: event.dx, dy: event.dy, scale: event.scale};
        }
        this._ensureFrameSource();
    }

    _queueAxisFactor(factor) {
        if (this._pending?.kind === 'axis')
            this._pending.factor *= factor;
        else
            this._pending = {kind: 'axis', factor};
        this._ensureFrameSource();
    }

    _ensureFrameSource() {
        if (this._frameSourceId !== 0)
            return;
        this._frameSourceId = GLib.timeout_add(
            GLib.PRIORITY_DEFAULT, FRAME_INTERVAL_MS, () => {
                this._frameSourceId = 0;
                this._flushPending();
                return GLib.SOURCE_REMOVE;
            });
    }

    _flushPending() {
        if (this._frameSourceId !== 0) {
            GLib.source_remove(this._frameSourceId);
            this._frameSourceId = 0;
        }
        const pending = this._pending;
        this._pending = null;
        if (!pending || !this._targetAlive())
            return;
        if (pending.kind === 'preview') {
            this._previewSwipe(pending.event);
        } else if (pending.kind === 'free') {
            this._applyFreeDelta(pending);
        } else if (pending.kind === 'axis') {
            this._applyAxisFactor(pending.factor);
        }
    }

    _discardPending() {
        if (this._frameSourceId !== 0) {
            GLib.source_remove(this._frameSourceId);
            this._frameSourceId = 0;
        }
        this._pending = null;
    }

    _previewSwipe(event) {
        const {direction, progress} = event;
        this._downMaxY = Math.max(this._downMaxY, this._lastDy);
        if (this._awaitingRebaseline) {
            if (direction === this._restoreDirection) {
                this._awaitingRebaseline = false;
            } else {
                this._showRestoreDirection();
                return;
            }
        }
        if (this._downLatched && this._handleDownPreview(direction))
            return;

        if (direction === 'none' || progress <= 0) {
            this._restoreLiveOriginal();
            this._livePreviewZone = null;
            this._hideHud(!this._systemAnimationsEnabled());
            return;
        }

        const zone = this._currentZone(direction);
        if (zone === 'minimize' && this._downMaxY < this._settings.swipeDownThreshold) {
            this._restoreLiveOriginal();
            this._hud.showBadge?.('Gesture', this._settings.overlayColor);
            return;
        }
        if (this._handleDownPreview(direction))
            return;
        if (zone === 'none') {
            this._livePreviewZone = null;
            this._hideSnapPreview(!this._systemAnimationsEnabled());
            return;
        }

        if (zone !== 'minimize' && direction !== this._priorDirection) {
            this._priorDirection = direction;
            this._priorDirectionSince = GLib.get_monotonic_time();
        }
        if (zone === this._livePreviewZone)
            return;

        if (isHalfOrQuarterZone(zone)) {
            this._restoreLiveOriginal();
            this._hud.showSnapPreview?.(
                zone, this._settings.overlayColor, this._workArea());
        } else if (this._settings.livePreview) {
            this._hideSnapPreview(true);
            if (zone === 'minimize') {
                this._restoreLiveOriginal();
                this._hud.showBadge?.(zoneLabel(zone), this._settings.overlayColor);
            } else {
                this._applyPreviewZone(zone);
                this._hud.showBadge?.(zoneLabel(zone), this._settings.overlayColor);
            }
        } else {
            this._hideSnapPreview(true);
            this._hideHud();
        }
        this._livePreviewZone = zone;
    }

    _handleDownPreview(direction) {
        if (!this._settings.minimizeEnabled ||
            this._settings.swipeDownAction === 'minimize')
            return false;

        if (this._downLatched) {
            this._downPeakY = Math.max(this._downPeakY, this._lastDy);
            if (this._lastDy < this._downPeakY - 0.05) {
                this._downLatched = false;
                this._downEngaged = false;
                this._downPickClose = false;
                this._downPeakY = 0;
                this._restoreLiveOriginal();
                this._livePreviewZone = null;
                this._hideHud();
                this._pendingRebaseline = this._restoreDirection;
                this._awaitingRebaseline = true;
                this._showRestoreDirection();
                return true;
            }
            if (direction === 'none') {
                this._downEngaged = false;
                this._hideHud();
                return true;
            }
            this._downEngaged = true;
            this._downPickClose = this._settings.swipeDownAction === 'close' ||
                (this._settings.swipeDownAction === 'choose' && this._lastDx > 0.045);
            this._restoreLiveOriginal();
            this._hud.showChooser?.(this._settings.swipeDownAction === 'choose',
                this._downPickClose, this._settings.overlayColor);
            return true;
        }

        if (this._currentZone(direction) !== 'minimize')
            return false;
        const now = GLib.get_monotonic_time();
        const dwellMs = this._priorDirectionSince === 0
            ? 0
            : (now - this._priorDirectionSince) / 1000;
        this._restoreDirection = this._priorDirection !== 'none' && dwellMs >= 150
            ? this._priorDirection
            : 'none';
        this._downLatched = true;
        this._downEngaged = true;
        this._downPeakY = this._lastDy;
        this._downPickClose = this._settings.swipeDownAction === 'close' ||
            (this._settings.swipeDownAction === 'choose' && this._lastDx > 0.045);
        this._restoreLiveOriginal();
        this._hud.showChooser?.(this._settings.swipeDownAction === 'choose',
            this._downPickClose, this._settings.overlayColor);
        return true;
    }

    _showRestoreDirection() {
        if (this._restoreDirection === 'none')
            return;
        const zone = standardZone(this._restoreDirection, this._settings);
        if (zone === 'none')
            return;
        if (isHalfOrQuarterZone(zone)) {
            this._restoreLiveOriginal();
            this._hud.showSnapPreview?.(
                zone, this._settings.overlayColor, this._workArea());
        } else if (this._settings.livePreview) {
            this._applyPreviewZone(zone);
        } else {
            this._hideHud();
        }
        this._livePreviewZone = zone;
    }

    _applyPreviewZone(zone) {
        if (zone === 'maximize') {
            if (this._target.can_maximize()) {
                if (this._originalMaximizeFlags === Meta.MaximizeFlags.BOTH) {
                    this._target.unmaximize();
                    this._maxRestored = true;
                } else {
                    this._target.maximize();
                }
                this._mutated = true;
            }
            return;
        }
        if (!this._target.allows_move() || !this._target.allows_resize())
            return;
        const rect = clampRect(
            zoneRect(this._workArea(), zone, this._settings.gridSpacing),
            this._workArea());
        this._prepareForTiledRect(rect);
        this._moveResize(rect);
    }

    _restoreLiveOriginal() {
        if (this._mutated)
            this._restoreOriginal();
    }

    _restorePreviewAndAccept() {
        this._restoreLiveOriginal();
        return true;
    }

    _showMonitorPreview(direction) {
        if (!direction || !this._settings.monitorMoveEnabled) {
            this._hideHud();
            return;
        }
        const neighbor = this._neighborMonitor(direction);
        if (neighbor < 0) {
            this._hideHud();
            return;
        }
        this._hud.showBadge?.(`Monitor ${direction}`, this._settings.overlayColor);
    }

    _applyZone(zone, moveCursor = false) {
        if (!this._targetAlive())
            return false;
        const adaptive = isHalfOrQuarterZone(zone);
        const interruptedVisual = adaptive
            ? this._takeCommittedPresentation(this._target)
            : null;
        if (!adaptive)
            this._cancelCommittedForTarget(this._target);
        this._hideSnapPreview(true);
        if (zone === 'maximize') {
            if (!this._target.can_maximize())
                return false;
            const before = copyRect(this._target.get_frame_rect());
            const animation = this._settings.animateSnaps
                ? this._prepareCompositorAnimation(this._target, before)
                : null;
            const actor = animation?.actor ?? this._target.get_compositor_private?.();
            if (actor)
                this._hud.skipNextWindowEffect?.(actor);
            if (typeof this._target.unminimize === 'function')
                this._target.unminimize();
            if (this._originalMaximizeFlags === Meta.MaximizeFlags.BOTH)
                this._target.unmaximize();
            else if (this._target.get_maximize_flags() !== Meta.MaximizeFlags.BOTH)
                this._target.maximize();
            if (animation)
                this._playAfterStateChange(animation, before);
            this._mutated = true;
            return true;
        }
        if (zone === 'minimize') {
            if (!this._target.can_minimize())
                return false;
            this._target.minimize();
            this._mutated = true;
            return true;
        }
        const work = this._workArea();
        const rect = clampRect(
            zoneRect(work, zone, this._settings.gridSpacing), work);
        const before = copyRect(this._target.get_frame_rect());
        if (this._target.get_maximize_flags() === Meta.MaximizeFlags.BOTH) {
            // Mutter can report a fully maximized window as temporarily not
            // movable/resizable even though both operations become available
            // as soon as the window is restored.  Checking those capabilities
            // before unmaximize made every snap action fail while minimize,
            // whose branch is above this check, continued to work.
            const accepted = this._commitFromFullyMaximized(rect, before, {
                adaptive,
                beforeVisual: interruptedVisual,
                work,
            });
            if (accepted && moveCursor && this._settings.moveCursor)
                this._moveCursorWithWindow(before, rect);
            return accepted;
        }
        if (!this._target.allows_move() || !this._target.allows_resize())
            return false;
        const animation = this._settings.animateSnaps &&
            (!adaptive || this._systemAnimationsEnabled())
            ? this._prepareCompositorAnimation(this._target, before,
                adaptiveAnimationOptions(interruptedVisual ?? before, rect, work, adaptive))
            : null;
        const preserveVerticalHalfState =
            (zone === 'leftHalf' || zone === 'rightHalf') &&
            this._target.get_maximize_flags() === Meta.MaximizeFlags.VERTICAL &&
            maximizeFlagsForRect(rect, work) === Meta.MaximizeFlags.VERTICAL;
        this._prepareForTiledRect(rect, preserveVerticalHalfState);
        this._moveResize(rect);
        if (animation)
            this._playCompositorAnimation(animation, rect);
        if (moveCursor && this._settings.moveCursor)
            this._moveCursorWithWindow(before, rect);
        return true;
    }

    _hideHud(immediate = false) {
        try {
            this._hud?.hide?.(immediate);
        } catch (error) {
            console.warn(`three-finger-drag HUD hide skipped: ${error.message}`);
        }
    }

    _hideSnapPreview(immediate = false) {
        try {
            this._hud?.hideSnapPreview?.(immediate);
        } catch (error) {
            console.warn(`three-finger-drag snap preview hide skipped: ${error.message}`);
        }
    }

    _systemAnimationsEnabled() {
        try {
            return this._desktopInterfaceSettings?.get_boolean('enable-animations') !== false;
        } catch (_error) {
            return true;
        }
    }

    _prepareCompositorAnimation(target, before, options = {}) {
        let actor;
        let frozen = false;
        try {
            actor = target.get_compositor_private?.();
            if (!actor || typeof actor.freeze !== 'function' ||
                typeof actor.thaw !== 'function' ||
                typeof actor.ease !== 'function')
                return null;

            const beforeVisual = copyRect(
                options.beforeVisual ?? actorVisualRect(actor, before));
            actor.remove_all_transitions?.();
            resetActorTransform(actor);
            actor.freeze();
            frozen = true;

            const record = {
                target,
                actor,
                before: copyRect(before),
                beforeVisual,
                duration: options.duration ?? Math.max(50,
                    Math.round(this._settings.snapAnimationSeconds * 1000)),
                mode: options.mode ?? Clutter.AnimationMode.EASE_OUT_CUBIC,
                adaptive: options.adaptive === true,
                started: false,
                frozen: true,
                actorDestroyId: 0,
            };
            record.actorDestroyId = actor.connect?.('destroy', () => {
                record.actorDestroyId = 0;
                this._cancelCommittedForTarget(record.target);
            }) ?? 0;
            this._committedAnimations.set(target, record);
            return record;
        } catch (error) {
            console.warn(`three-finger-drag: compositor animation unavailable: ${error.message}`);
            try {
                if (frozen)
                    actor?.thaw?.();
                resetActorTransform(actor);
            } catch (_error) {
                // The actor may already have been destroyed.
            }
            return null;
        }
    }

    _playCompositorAnimation(record, after) {
        if (this._committedAnimations.get(record.target) !== record)
            return;
        const duration = record.duration;
        const source = record.beforeVisual;
        const target = visualDestinationRect(record.actor, source,
            record.before, roundedRect(after));
        const inverseScaleX = source.width / Math.max(1, target.width);
        const inverseScaleY = source.height / Math.max(1, target.height);

        try {
            record.actor.translation_x = source.x - target.x;
            record.actor.translation_y = source.y - target.y;
            record.actor.scale_x = inverseScaleX;
            record.actor.scale_y = inverseScaleY;
            record.started = true;
            record.actor.ease({
                translation_x: 0,
                translation_y: 0,
                scale_x: 1,
                scale_y: 1,
                duration,
                mode: record.mode,
                onStopped: () => this._finishCompositorAnimation(record, true),
            });
            record.actor.thaw();
            record.frozen = false;
        } catch (_error) {
            this._finishCompositorAnimation(record, true);
        }
    }

    _playAfterStateChange(record, before) {
        let attempts = 0;
        let sourceId = 0;
        sourceId = GLib.timeout_add(
            GLib.PRIORITY_DEFAULT,
            MAXIMIZED_SETTLE_INTERVAL_MS,
            () => {
                attempts++;
                try {
                    const after = copyRect(record.target.get_frame_rect());
                    if (!sameRect(before, after) ||
                        attempts >= MAXIMIZED_SETTLE_ATTEMPTS) {
                        this._forgetCommittedSource(sourceId);
                        this._playCompositorAnimation(record, after);
                        return GLib.SOURCE_REMOVE;
                    }
                } catch (_error) {
                    this._forgetCommittedSource(sourceId);
                    this._finishCompositorAnimation(record, true);
                    return GLib.SOURCE_REMOVE;
                }
                return GLib.SOURCE_CONTINUE;
            });
        this._trackCommittedSource(record.target, sourceId);
    }

    _finishCompositorAnimation(record, resetActor) {
        if (this._committedAnimations.get(record.target) !== record)
            return;
        this._committedAnimations.delete(record.target);
        try {
            if (record.actorDestroyId)
                record.actor.disconnect(record.actorDestroyId);
        } catch (_error) {
            // The actor may already be destroyed.
        }
        record.actorDestroyId = 0;
        try {
            if (record.frozen)
                record.actor.thaw();
            if (resetActor)
                resetActorTransform(record.actor);
        } catch (_error) {
            // No actor state remains to clean after destruction.
        }
        record.frozen = false;
    }

    _takeCommittedPresentation(target) {
        const record = this._committedAnimations.get(target);
        const presentation = record
            ? this._stopCommittedAnimation(record, true)
            : null;
        this._removeCommittedSourcesForTarget(target);
        return presentation;
    }

    _cancelCommittedForTarget(target, options = {}) {
        const record = this._committedAnimations.get(target);
        const hasDeferredSources = [...this._sourceTargets.values()]
            .some(sourceTarget => sourceTarget === target);
        const preserveRunningAdaptive = options.preserveRunningAdaptive === true &&
            record?.adaptive === true && record.started === true && !hasDeferredSources;
        if (record && !preserveRunningAdaptive)
            this._stopCommittedAnimation(record, false);
        this._removeCommittedSourcesForTarget(target);
    }

    _stopCommittedAnimation(record, capturePresentation) {
        if (this._committedAnimations.get(record.target) !== record)
            return null;
        const presentation = capturePresentation
            ? actorVisualRect(record.actor, record.before)
            : null;

        // Detach first: Clutter may synchronously invoke the old onStopped
        // callback while transitions are removed. That callback must observe
        // a stale record and never reset a newer FLIP prepared in this frame.
        this._committedAnimations.delete(record.target);
        try {
            if (record.actorDestroyId)
                record.actor.disconnect(record.actorDestroyId);
        } catch (_error) {
            // The actor may already be destroyed.
        }
        record.actorDestroyId = 0;
        try {
            record.actor.remove_all_transitions?.();
            if (record.frozen)
                record.actor.thaw();
            resetActorTransform(record.actor);
        } catch (_error) {
            // No actor state remains to clean after destruction.
        }
        record.frozen = false;
        return presentation ? copyRect(presentation) : null;
    }

    _removeCommittedSourcesForTarget(target) {
        for (const [sourceId, sourceTarget] of [...this._sourceTargets]) {
            if (sourceTarget !== target)
                continue;
            GLib.source_remove(sourceId);
            this._committedSources.delete(sourceId);
            this._sourceTargets.delete(sourceId);
        }
    }

    _trackCommittedSource(target, sourceId) {
        this._committedSources.add(sourceId);
        this._sourceTargets.set(sourceId, target);
    }

    _forgetCommittedSource(sourceId) {
        this._committedSources.delete(sourceId);
        this._sourceTargets.delete(sourceId);
    }

    _moveCursorWithWindow(before, after) {
        const [pointerX, pointerY] = global.get_pointer();
        const point = cursorAfterWindowMove(
            {x: pointerX, y: pointerY}, before, after);
        try {
            Clutter.get_default_backend().get_default_seat().warp_pointer(point.x, point.y);
        } catch (error) {
            console.warn(`three-finger-drag: unable to move cursor with window: ${error.message}`);
        }
    }

    _commitFromFullyMaximized(rect, before, options = {}) {
        const target = this._target;
        const work = copyRect(options.work ?? this._workArea());
        const rounded = roundedRect(rect);
        const desiredFlags = maximizeFlagsForRect(rounded, work);
        const adaptive = options.adaptive === true;
        const animation = this._settings.animateSnaps &&
            (!adaptive || this._systemAnimationsEnabled())
            ? this._prepareCompositorAnimation(target, before,
                adaptiveAnimationOptions(
                    options.beforeVisual ?? before, rounded, work, adaptive))
            : null;

        if (animation)
            this._hud.skipNextWindowEffect?.(animation.actor);

        if (typeof target.unmake_fullscreen === 'function')
            target.unmake_fullscreen();
        target.unmaximize();
        if (typeof target.unmake_above === 'function')
            target.unmake_above();
        if (typeof target.unminimize === 'function')
            target.unminimize();
        target.move_to_monitor(target.get_monitor());

        this._currentRect = rounded;
        this._mutated = true;
        let attempts = 0;
        const sourceId = GLib.timeout_add(
            GLib.PRIORITY_DEFAULT,
            MAXIMIZED_SETTLE_INTERVAL_MS,
            () => {
                attempts++;
                try {
                    target.get_frame_rect();
                    // A full maximize transition may not be committed until
                    // the next Mutter frame. Do not race a geometry write
                    // against BOTH flags; repeat unmaximize until it lands.
                    if (target.get_maximize_flags() === Meta.MaximizeFlags.BOTH) {
                        target.unmaximize();
                    } else if (!target.allows_move() || !target.allows_resize()) {
                        // The maximize transition and the corresponding move/
                        // resize capabilities do not always settle in the same
                        // Mutter frame. Retry briefly, then restore the safe
                        // original state instead of leaving a half-restored
                        // non-resizable window behind.
                        if (attempts >= MAXIMIZED_SETTLE_ATTEMPTS) {
                            target.maximize();
                            console.warn('three-finger-drag: restored maximized window ' +
                                'never became movable/resizable; original state restored');
                            this._forgetCommittedSource(sourceId);
                            if (animation)
                                this._finishCompositorAnimation(animation, true);
                            return GLib.SOURCE_REMOVE;
                        }
                    } else {
                        applyMaximizeFlags(target, desiredFlags);
                        moveResizeTarget(target, rounded);
                        if (rectMatches(target.get_frame_rect(), rounded) ||
                            attempts >= MAXIMIZED_SETTLE_ATTEMPTS) {
                            if (attempts >= MAXIMIZED_SETTLE_ATTEMPTS &&
                                !rectMatches(target.get_frame_rect(), rounded)) {
                                console.warn('three-finger-drag: maximized window did not ' +
                                    `settle at ${formatRect(rounded)}; actual=` +
                                    formatRect(target.get_frame_rect()));
                            }
                            this._forgetCommittedSource(sourceId);
                            if (animation)
                                this._playCompositorAnimation(animation, rounded);
                            return GLib.SOURCE_REMOVE;
                        }
                    }
                } catch (_error) {
                    this._forgetCommittedSource(sourceId);
                    if (animation)
                        this._finishCompositorAnimation(animation, true);
                    return GLib.SOURCE_REMOVE;
                }
                if (attempts >= MAXIMIZED_SETTLE_ATTEMPTS) {
                    this._forgetCommittedSource(sourceId);
                    if (animation)
                        this._finishCompositorAnimation(animation, true);
                    return GLib.SOURCE_REMOVE;
                }
                return GLib.SOURCE_CONTINUE;
            });
        this._trackCommittedSource(target, sourceId);
        return true;
    }

    _prepareForTiledRect(rect, preserveMaximizeState = false) {
        const target = this._target;
        const work = this._workArea();
        const wasMaximized = target.get_maximize_flags() !== 0;

        // Mutter 50 does not reliably accept a horizontal resize while the
        // window still carries BOTH maximize flags.  Match Ubuntu's bundled
        // Tiling Assistant: leave fullscreen/maximized state first, then keep
        // only the axis that the destination fills before moving the frame.
        if (typeof target.unmake_fullscreen === 'function')
            target.unmake_fullscreen();
        if (wasMaximized && !preserveMaximizeState)
            target.unmaximize();
        if (typeof target.unmake_above === 'function')
            target.unmake_above();
        if (typeof target.unminimize === 'function')
            target.unminimize();

        // Moving between left and right halves keeps the same VERTICAL state.
        // Releasing and immediately reapplying it sends two asynchronous
        // Wayland configurations; a late restore can then pull the window back.
        if (!preserveMaximizeState)
            applyMaximizeFlags(target, maximizeFlagsForRect(rect, work));

        // Explicitly pin the operation to the selected window's monitor. This
        // is also part of Ubuntu's native tiling sequence and avoids Mutter
        // clamping a right-half move back to the old maximized frame.
        target.move_to_monitor(target.get_monitor());
    }

    _applyDownAction() {
        const action = this._settings.swipeDownAction;
        if (action === 'close')
            return this._closeWindow();
        if (action === 'choose') {
            // Horizontal intent provides an in-gesture chooser without a
            // modal dialog: left/default minimizes, right closes gracefully.
            return this._downChoice() === 'close'
                ? this._closeWindow()
                : this._applyZone('minimize');
        }
        return this._applyZone('minimize');
    }

    _downChoice() {
        return this._lastDx > 0.045
            ? 'close'
            : 'minimize';
    }

    _closeWindow() {
        if (!this._targetAlive())
            return false;
        if (typeof this._target.can_close === 'function' && !this._target.can_close())
            return false;
        // Graceful compositor close only. Never kill a client process.
        this._target.delete(global.get_current_time());
        this._mutated = true;
        return true;
    }

    _applyFreeDelta({dx, dy, scale}) {
        if (!this._target.allows_move())
            return;
        const work = this._workArea();
        const old = this._currentRect ?? copyRect(this._target.get_frame_rect());
        const width = old.width * scale;
        const height = old.height * scale;
        const next = clampRect({
            x: old.x + dx * work.width - (width - old.width) / 2,
            y: old.y + dy * work.height - (height - old.height) / 2,
            width,
            height,
        }, work);
        if (!this._target.allows_resize()) {
            next.width = old.width;
            next.height = old.height;
        }
        this._moveResize(next);
        this._hud.showBadge?.('Move / resize', this._settings.overlayColor);
    }

    _applyAxisFactor(factor) {
        if (!this._target.allows_resize())
            return;
        const old = this._currentRect ?? copyRect(this._target.get_frame_rect());
        const width = this._axisHorizontal ? old.width * factor : old.width;
        const height = this._axisHorizontal ? old.height : old.height * factor;
        const next = clampRect({
            x: old.x - (width - old.width) / 2,
            y: old.y - (height - old.height) / 2,
            width,
            height,
        }, this._workArea());
        this._moveResize(next);
        this._hud.showBadge?.(this._axisHorizontal ? 'Width' : 'Height',
            this._settings.overlayColor);
    }

    _moveResize(rect) {
        const rounded = roundedRect(rect);
        moveResizeTarget(this._target, rounded);
        this._currentRect = rounded;
        this._mutated = true;
    }

    _moveToMonitor(direction) {
        if (!this._settings.monitorMoveEnabled)
            return false;
        this._cancelCommittedForTarget(this._target);
        const source = this._target.get_monitor();
        const destination = this._neighborMonitor(direction);
        if (destination < 0)
            return false;
        const sourceWork = this._target.get_work_area_for_monitor(source);
        const destinationWork = this._target.get_work_area_for_monitor(destination);
        const rect = copyRect(this._target.get_frame_rect());
        const animation = this._settings.animateSnaps &&
            this._target.get_maximize_flags() === 0
            ? this._prepareCompositorAnimation(this._target, rect)
            : null;
        const relativeX = sourceWork.width > rect.width
            ? (rect.x - sourceWork.x) / (sourceWork.width - rect.width)
            : 0;
        const relativeY = sourceWork.height > rect.height
            ? (rect.y - sourceWork.y) / (sourceWork.height - rect.height)
            : 0;
        this._target.move_to_monitor(destination);
        if (this._target.get_maximize_flags() === 0) {
            this._target.move_frame(true,
                Math.round(destinationWork.x + relativeX *
                    Math.max(0, destinationWork.width - rect.width)),
                Math.round(destinationWork.y + relativeY *
                    Math.max(0, destinationWork.height - rect.height)));
        }
        if (animation)
            this._playCompositorAnimation(animation, this._target.get_frame_rect());
        this._mutated = true;
        return true;
    }

    _beginHoldMode() {
        this._holdAimSteps = 0;
        this._appSwitchActive = false;
        this._appWindows = [];
        if (this._settings.appSwitchOnHold) {
            this._appWindows = this._enumerateSwitchableWindows();
            if (!this._appWindows.includes(this._target))
                this._appWindows.unshift(this._target);
            this._appStart = Math.max(0, this._appWindows.indexOf(this._target));
            this._appSelected = this._appStart;
            this._appSwitchActive = this._appWindows.length > 0;
        }
        this._updateHoldPreview();
    }

    _enumerateSwitchableWindows() {
        const workspace = global.workspace_manager.get_active_workspace();
        return global.display.sort_windows_by_stacking(workspace.list_windows())
            .reverse()
            .filter(window => {
                try {
                    return !window.is_skip_taskbar() && !window.is_override_redirect() &&
                        window.get_window_type() === Meta.WindowType.NORMAL;
                } catch (_error) {
                    return false;
                }
            });
    }

    _updateHoldPreview() {
        if (this._appSwitchActive) {
            this._appSelected = appSelectionIndex(
                this._appStart, this._holdAimSteps, this._appWindows.length);
            const visibleCount = Math.min(5, this._appWindows.length);
            const visibleStart = Math.max(0, Math.min(
                this._appSelected - Math.floor(visibleCount / 2),
                this._appWindows.length - visibleCount));
            const visible = this._appWindows
                .slice(visibleStart, visibleStart + visibleCount).map(window => {
                try {
                    return window.get_title() || window.get_wm_class() || 'Application';
                } catch (_error) {
                    return 'Application';
                }
            });
            this._hud.showStrip?.(visible, this._appSelected - visibleStart,
                'Applications', this._settings.overlayColor);
            return;
        }
        const manager = global.workspace_manager;
        const count = manager.get_n_workspaces();
        const current = manager.get_active_workspace_index();
        const aimed = current + this._holdAimSteps;
        const labels = Array.from({length: Math.min(10, count)},
            (_value, index) => String(index + 1));
        if (this._settings.createDesktopOnOverflow && aimed >= count && labels.length < 10)
            labels.push('+');
        const selected = aimed >= 0 && aimed < labels.length ? aimed : current;
        this._hud.showStrip?.(labels, selected, 'Workspaces', this._settings.overlayColor);
    }

    _commitAppSwitch(steps) {
        if (!this._appSwitchActive || this._appWindows.length === 0)
            return false;
        const selectedIndex = appSelectionIndex(
            this._appStart, steps, this._appWindows.length);
        const selected = this._appWindows[selectedIndex];
        if (selected === this._target)
            return true;
        try {
            if (typeof selected.unminimize === 'function')
                selected.unminimize();
            if (this._originalWorkspace && selected.get_workspace() !== this._originalWorkspace)
                selected.change_workspace(this._originalWorkspace);
            if (this._originalMonitor >= 0)
                selected.move_to_monitor(this._originalMonitor);
            if (this._originalMaximizeFlags !== 0) {
                if (typeof selected.set_maximize_flags === 'function')
                    selected.set_maximize_flags(this._originalMaximizeFlags);
                else
                    selected.maximize();
            } else {
                selected.unmaximize();
                if (selected.allows_move() && selected.allows_resize())
                    moveResizeTarget(selected, this._original);
            }
            const workspace = this._originalWorkspace ?? selected.get_workspace();
            workspace.activate_with_focus(selected, global.get_current_time());
            return true;
        } catch (error) {
            console.warn(`three-finger-drag: app switch failed: ${error.message}`);
            return false;
        }
    }

    _moveToWorkspace(direction, steps, committedImmediately = false) {
        let workspace = this._target.get_workspace();
        const metaDirection = direction === 'left'
            ? Meta.MotionDirection.LEFT
            : Meta.MotionDirection.RIGHT;
        for (let index = 0; index < steps; index++) {
            const neighbor = workspace.get_neighbor(metaDirection);
            if (neighbor !== workspace) {
                workspace = neighbor;
                continue;
            }
            if (direction !== 'right' || !this._settings.createDesktopOnOverflow)
                break;
            const manager = global.workspace_manager;
            const before = manager.get_n_workspaces();
            manager.append_new_workspace(false, global.get_current_time());
            const after = manager.get_n_workspaces();
            if (after <= before)
                break;
            workspace = manager.get_workspace_by_index(after - 1);
        }
        if (workspace === this._target.get_workspace())
            // Existing-workspace capability deliberately stops at the edge.
            // A boundary is an accepted no-op, not a malformed Update: false
            // would make the broker cancel and restore earlier valid steps.
            return true;
        this._target.change_workspace(workspace);
        workspace.activate_with_focus(this._target, global.get_current_time());
        this._mutated = true;
        if (committedImmediately) {
            // Windows commits each DesktopMove as it is emitted; the later
            // hold-release Cancelled event only closes the gesture and must not
            // undo an already completed workspace step.
            this._originalWorkspace = workspace;
            this._originalWorkspaceIndex = workspace.index();
            this._originalMonitor = this._target.get_monitor();
            this._original = copyRect(this._target.get_frame_rect());
            this._originalMaximizeFlags = this._target.get_maximize_flags();
            this._mutated = false;
        }
        return true;
    }

    _neighborMonitor(direction) {
        const directions = {
            left: Meta.DisplayDirection.LEFT,
            right: Meta.DisplayDirection.RIGHT,
            up: Meta.DisplayDirection.UP,
            down: Meta.DisplayDirection.DOWN,
        };
        return global.display.get_monitor_neighbor_index(
            this._target.get_monitor(), directions[direction]);
    }

    _restoreOriginal() {
        if (!this._targetAlive())
            return false;

        const workspace = this._restorableWorkspace();
        if (workspace && this._target.get_workspace() !== workspace) {
            this._target.change_workspace(workspace);
            workspace.activate_with_focus(this._target, global.get_current_time());
        }
        if (this._originalMonitor >= 0 &&
            this._originalMonitor < global.display.get_n_monitors() &&
            this._target.get_monitor() !== this._originalMonitor)
            this._target.move_to_monitor(this._originalMonitor);

        if (this._originalMaximizeFlags !== 0) {
            if (typeof this._target.set_maximize_flags === 'function')
                this._target.set_maximize_flags(this._originalMaximizeFlags);
            else
                this._target.maximize();
        } else {
            this._target.unmaximize();
            if (this._target.allows_move() && this._target.allows_resize())
                this._moveResize(this._original);
        }
        this._mutated = false;
        return true;
    }

    _restorableWorkspace() {
        try {
            if (this._originalWorkspace?.index() >= 0)
                return this._originalWorkspace;
        } catch (_error) {
            // A dynamic workspace can disappear after the target moves away.
        }
        const count = global.workspace_manager.get_n_workspaces();
        if (count === 0 || this._originalWorkspaceIndex < 0)
            return null;
        return global.workspace_manager.get_workspace_by_index(
            Math.min(this._originalWorkspaceIndex, count - 1));
    }

    _axisAllowed(horizontal) {
        return horizontal
            ? this._settings.resizeHorizontalEnabled
            : this._settings.resizeVerticalEnabled;
    }

    _workArea() {
        return this._target.get_work_area_current_monitor();
    }

    _targetAlive() {
        if (this._target === null)
            return false;
        try {
            // Meta.Window.is_alive() is a client responsiveness/liveness hint,
            // not the lifetime of the Shell-side window object. Treating it as
            // object existence makes valid CSD/Wayland windows accept Begin
            // and then reject the very first Update. The `unmanaging` signal
            // clears our target authoritatively; this read guards races with
            // that signal without rejecting an unresponsive client.
            this._target.get_frame_rect();
            return true;
        } catch (_error) {
            return false;
        }
    }

    _disconnectTarget() {
        if (this._target && this._targetUnmanagingId !== 0) {
            try {
                this._target.disconnect(this._targetUnmanagingId);
            } catch (_error) {
                // The target may already be unmanaged.
            }
        }
        this._targetUnmanagingId = 0;
    }

    _resetState() {
        this._settings = null;
        this._target = null;
        this._targetUnmanagingId = 0;
        this._mode = null;
        this._original = null;
        this._originalMaximizeFlags = 0;
        this._originalWorkspace = null;
        this._originalWorkspaceIndex = -1;
        this._originalMonitor = -1;
        this._currentRect = null;
        this._lastDx = 0;
        this._lastDy = 0;
        this._axisHorizontal = false;
        this._mutated = false;
        this._maxRestored = false;
        this._livePreviewZone = null;
        this._downLatched = false;
        this._downEngaged = false;
        this._downPickClose = false;
        this._downPeakY = 0;
        this._downMaxY = 0;
        this._priorDirection = 'none';
        this._priorDirectionSince = 0;
        this._restoreDirection = 'none';
        this._pendingRebaseline = null;
        this._awaitingRebaseline = false;
        this._holdAimSteps = 0;
        this._appSwitchActive = false;
        this._appWindows = [];
        this._appStart = 0;
        this._appSelected = 0;
        this._pending = null;
        this._frameSourceId = 0;
        this._lastTargetDetail = 'pointer is not over a manageable titlebar';
    }
}

function pointInTopBand(x, y, rect, bandHeight) {
    return x >= rect.x && x < rect.x + rect.width &&
        y >= rect.y && y <= rect.y + bandHeight;
}

function formatRect(rect) {
    return `${Math.round(rect.x)},${Math.round(rect.y)},` +
        `${Math.round(rect.width)}x${Math.round(rect.height)}`;
}

function roundedRect(rect) {
    return Object.fromEntries(Object.entries(rect)
        .map(([key, value]) => [key, Math.round(value)]));
}

function adaptiveAnimationOptions(source, target, work, adaptive) {
    if (!adaptive)
        return {};
    return {
        beforeVisual: copyRect(source),
        duration: adaptiveSnapDuration(source, target, work),
        mode: Clutter.AnimationMode.EASE_OUT_QUART,
        adaptive: true,
    };
}

function maximizeFlagsForRect(rect, work) {
    const fillsWidth = rect.x === work.x && rect.width === work.width;
    const fillsHeight = rect.y === work.y && rect.height === work.height;
    if (fillsWidth && !fillsHeight)
        return Meta.MaximizeFlags.HORIZONTAL;
    if (fillsHeight && !fillsWidth)
        return Meta.MaximizeFlags.VERTICAL;
    return 0;
}

function applyMaximizeFlags(target, flags) {
    if (flags !== 0 && typeof target.set_maximize_flags === 'function')
        target.set_maximize_flags(flags);
}

function moveResizeTarget(target, rect) {
    target.move_frame(true, rect.x, rect.y);
    target.move_resize_frame(true, rect.x, rect.y, rect.width, rect.height);
}

function resetActorTransform(actor) {
    actor.translation_x = 0;
    actor.translation_y = 0;
    actor.scale_x = 1;
    actor.scale_y = 1;
}

function actorVisualRect(actor, fallback) {
    try {
        const [x, y] = actor.get_transformed_position();
        const [width, height] = actor.get_transformed_size();
        if ([x, y, width, height].every(Number.isFinite) && width > 0 && height > 0)
            return {x, y, width, height};
    } catch (_error) {
        // Tests and older compositor actors can use frame geometry as a safe
        // fallback; the final Meta.Window rectangle remains authoritative.
    }
    return copyRect(fallback);
}

function visualDestinationRect(actor, sourceVisual, sourceFrame, targetFrame) {
    const actual = actorVisualRect(actor, targetFrame);
    if (!sameRect(sourceFrame, targetFrame) && sameRect(actual, sourceVisual)) {
        // A synchronous Meta geometry write can precede the actor allocation
        // update by one compositor frame. Preserve the source shadow margins
        // so the first rendered animation frame still starts pixel-perfectly.
        return {
            x: targetFrame.x + sourceVisual.x - sourceFrame.x,
            y: targetFrame.y + sourceVisual.y - sourceFrame.y,
            width: Math.max(1, targetFrame.width + sourceVisual.width - sourceFrame.width),
            height: Math.max(1, targetFrame.height + sourceVisual.height - sourceFrame.height),
        };
    }
    return actual;
}

function sameRect(left, right) {
    return left.x === right.x && left.y === right.y &&
        left.width === right.width && left.height === right.height;
}

export function cursorAfterWindowMove(point, before, after) {
    const dx = point.x - before.x;
    const dy = point.y - before.y;
    return {
        x: Math.max(after.x, Math.min(after.x + after.width - 1, after.x + dx)),
        y: Math.max(after.y, Math.min(after.y + after.height - 1, after.y + dy)),
    };
}

export function appSelectionIndex(start, steps, count) {
    if (count <= 0)
        return 0;
    return Math.max(0, Math.min(count - 1, start + steps));
}

export function minimizeAllWindows(windows) {
    let minimized = 0;
    for (const window of windows) {
        try {
            if (window.minimized || window.is_skip_taskbar() ||
                window.get_window_type() !== Meta.WindowType.NORMAL ||
                !window.can_minimize())
                continue;
            window.minimize();
            minimized++;
        } catch (error) {
            console.warn(`three-finger-drag: unable to minimize a window: ${error.message}`);
        }
    }
    return minimized;
}

function rectMatches(actual, expected) {
    // GTK clients can quantize dimensions by a few logical pixels.
    const epsilon = 6;
    return Math.abs(actual.x - expected.x) <= epsilon &&
        Math.abs(actual.y - expected.y) <= epsilon &&
        Math.abs(actual.width - expected.width) <= epsilon &&
        Math.abs(actual.height - expected.height) <= epsilon;
}

export function modifierPressed(name) {
    const masks = {
        shift: Clutter.ModifierType.SHIFT_MASK,
        ctrl: Clutter.ModifierType.CONTROL_MASK,
        alt: Clutter.ModifierType.MOD1_MASK,
    };
    return Boolean(global.get_pointer()[2] & (masks[name] ?? 0));
}

function identifierVariants(value) {
    const normalized = value.trim().toLowerCase();
    if (!normalized)
        return [];
    return normalized.endsWith('.desktop')
        ? [normalized, normalized.slice(0, -'.desktop'.length)]
        : [normalized, `${normalized}.desktop`];
}

function zoneLabel(zone) {
    const labels = {
        none: '',
        maximize: 'Maximize',
        minimize: 'Minimize',
        leftHalf: 'Left half',
        rightHalf: 'Right half',
        topLeft: 'Top left',
        topRight: 'Top right',
        bottomLeft: 'Bottom left',
        bottomRight: 'Bottom right',
    };
    return labels[zone] ?? zone;
}
