export const PROTOCOL_VERSION = 1;
export const BUS_NAME = 'io.github.xuzenghui942.ThreeFingerDrag.Gnome';
export const OBJECT_PATH = '/io/github/xuzenghui942/ThreeFingerDrag/Gnome';
export const INTERFACE_NAME = 'io.github.xuzenghui942.ThreeFingerDrag.Gnome1';

export function makeCapabilities(generation) {
    const contactArbitration = {
        // The Rust evdev/uinput proxy owns the physical two-finger stream
        // before Mutter can dispatch it. The Shell extension only executes
        // the already-arbitrated window transaction.
        twoFinger: true,
        fiveFinger: false,
        threeFinger: true,
        fourFinger: false,
    };
    return {
        protocolVersion: PROTOCOL_VERSION,
        generation,
        advancedEvents: true,
        modifiers: true,
        rebaselineFeedback: true,
        contactArbitration,
        actions: {
            snap: true,
            snapHalves: true,
            snapQuarters: true,
            maximize: true,
            minimize: true,
            minimizeAll: true,
            close: true,
            workspace: true,
            dynamicWorkspace: true,
            monitorMove: true,
            freeMove: false,
            freeResize: false,
            axisResize: true,
            pinch: true,
            hud: true,
            animation: true,
            livePreview: true,
            snapPreview: true,
            adaptiveSnapAnimation: true,
            moveCursor: true,
            appSwitch: true,
        },
    };
}

const MAX_JSON_BYTES = 64 * 1024;
const SESSION_ID_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/;
const FORBIDDEN_TARGET_KEYS = new Set([
    'windowId',
    'window_id',
    'targetId',
    'target_id',
    'pid',
]);

export const UPDATE_KINDS = new Set([
    'raw',
    'updated',
    'holdEngaged',
    'holdUpdated',
    'monitorMoveUpdated',
    'freeMoveDelta',
    'pinchUpdated',
    'axisResizeBegan',
    'axisResizeDelta',
    'desktopMove',
]);

export const COMMIT_KINDS = new Set([
    'completed',
    'desktopHoldCommit',
    'monitorMove',
    'freeMoveEnded',
    'pinchOut',
    'pinchIn',
    'axisResizeEnded',
]);

const DIRECTIONS = new Set([
    'none', 'left', 'right', 'up', 'down',
    'upLeft', 'upRight', 'downLeft', 'downRight',
]);
const CARDINAL_DIRECTIONS = new Set(['left', 'right', 'up', 'down']);

function isPlainObject(value) {
    return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function finiteNumber(value, minimum = -Infinity, maximum = Infinity) {
    return typeof value === 'number' && Number.isFinite(value) &&
        value >= minimum && value <= maximum;
}

function containsForbiddenTarget(value) {
    if (Array.isArray(value))
        return value.some(containsForbiddenTarget);
    if (!isPlainObject(value))
        return false;
    return Object.entries(value).some(([key, child]) =>
        FORBIDDEN_TARGET_KEYS.has(key) || containsForbiddenTarget(child));
}

export function parseJsonObject(text, label) {
    if (typeof text !== 'string' || text.length === 0 || text.length > MAX_JSON_BYTES)
        throw new Error(`${label} size is invalid`);
    let parsed;
    try {
        parsed = JSON.parse(text);
    } catch (_error) {
        throw new Error(`${label} is not valid JSON`);
    }
    if (!isPlainObject(parsed))
        throw new Error(`${label} must be an object`);
    if (containsForbiddenTarget(parsed))
        throw new Error(`${label} contains a forbidden target identifier`);
    return parsed;
}

export function parseConfig(text) {
    const config = parseConfigEnvelope(text);
    if (config.advancedEnabled !== true)
        throw new Error('advanced mode is not explicitly enabled');
    return config;
}

export function parseConfigEnvelope(text) {
    const config = parseJsonObject(text, 'config');
    if (config.version !== PROTOCOL_VERSION ||
        typeof config.advancedEnabled !== 'boolean')
        throw new Error('config version or advancedEnabled is invalid');
    if (!isPlainObject(config.settings))
        throw new Error('config.settings must be an object');
    return config;
}

export function resolveConfiguration(config, currentSender, requestSender) {
    if (currentSender !== null && currentSender !== requestSender)
        throw new Error('another D-Bus owner configured the broker');
    return {
        sender: currentSender ?? requestSender,
        settings: config.advancedEnabled
            ? normalizeSettings(config.settings)
            : null,
    };
}

export function validateSessionId(sessionId) {
    return typeof sessionId === 'string' && SESSION_ID_PATTERN.test(sessionId);
}

export function sequenceToNumber(sequence) {
    const value = typeof sequence === 'bigint' ? Number(sequence) : Number(sequence);
    if (!Number.isSafeInteger(value) || value < 1)
        throw new Error('sequence must be a positive safe integer');
    return value;
}

export function parseEvent(text, allowedKinds) {
    const event = parseJsonObject(text, 'event');
    if (event.version !== PROTOCOL_VERSION || typeof event.kind !== 'string')
        throw new Error('event version or kind is invalid');
    if (!allowedKinds.has(event.kind))
        throw new Error(`event kind ${event.kind} is not valid here`);

    switch (event.kind) {
    case 'began':
        if (!Number.isInteger(event.contacts) || ![2, 3, 5].includes(event.contacts))
            throw new Error('began.contacts must be 2, 3, or 5');
        break;
    case 'freeMoveBegan':
    case 'holdEngaged':
    case 'pinchOut':
    case 'pinchIn':
        break;
    case 'raw':
        requireDelta(event);
        break;
    case 'updated':
        requireDirection(event.direction, DIRECTIONS);
        requireProgress(event.progress);
        break;
    case 'completed':
        requireDirection(event.direction, DIRECTIONS);
        break;
    case 'holdUpdated':
        if (event.direction !== null)
            requireDirection(event.direction, new Set(['left', 'right']));
        requireProgress(event.progress);
        if (!Number.isInteger(event.aimSteps) || Math.abs(event.aimSteps) > 64)
            throw new Error('holdUpdated.aimSteps is invalid');
        break;
    case 'desktopMove':
        requireDirection(event.direction, new Set(['left', 'right']));
        break;
    case 'desktopHoldCommit':
        if (!Number.isInteger(event.steps) || Math.abs(event.steps) > 64)
            throw new Error('desktopHoldCommit.steps is invalid');
        break;
    case 'monitorMoveUpdated':
        if (event.direction !== null)
            requireDirection(event.direction, CARDINAL_DIRECTIONS);
        requireProgress(event.progress);
        break;
    case 'monitorMove':
        requireDirection(event.direction, CARDINAL_DIRECTIONS);
        break;
    case 'freeMoveDelta':
        requireDelta(event);
        if (!finiteNumber(event.scale, 0.25, 4))
            throw new Error('freeMoveDelta.scale is invalid');
        break;
    case 'freeMoveEnded':
        requireBoolean(event.wasTap, 'freeMoveEnded.wasTap');
        requireBoolean(event.cancelled, 'freeMoveEnded.cancelled');
        break;
    case 'pinchUpdated':
        requireBoolean(event.outward, 'pinchUpdated.outward');
        requireProgress(event.progress);
        break;
    case 'axisResizeBegan':
        requireBoolean(event.horizontal, 'axisResizeBegan.horizontal');
        break;
    case 'axisResizeDelta':
        requireBoolean(event.horizontal, 'axisResizeDelta.horizontal');
        if (!finiteNumber(event.factor, 0.25, 4))
            throw new Error('axisResizeDelta.factor is invalid');
        break;
    case 'axisResizeEnded':
        requireBoolean(event.cancelled, 'axisResizeEnded.cancelled');
        break;
    default:
        throw new Error('unhandled event kind');
    }

    return event;
}

function requireBoolean(value, label) {
    if (typeof value !== 'boolean')
        throw new Error(`${label} must be a boolean`);
}

function requireDelta(event) {
    if (!finiteNumber(event.dx, -8, 8) || !finiteNumber(event.dy, -8, 8))
        throw new Error(`${event.kind} delta is invalid`);
}

function requireDirection(direction, allowed) {
    if (!allowed.has(direction))
        throw new Error('direction is invalid');
}

function requireProgress(progress) {
    if (!finiteNumber(progress, 0, 1.5))
        throw new Error('progress is invalid');
}

export function normalizeSettings(settings) {
    return {
        animateSnaps: settings.animateSnaps !== false,
        snapAnimationSeconds: finiteNumber(settings.snapAnimationSeconds, 0.05, 0.4)
            ? settings.snapAnimationSeconds
            : 0.22,
        maximizeEnabled: settings.maximizeEnabled !== false,
        halvesEnabled: settings.halvesEnabled !== false,
        quartersEnabled: settings.quartersEnabled !== false,
        minimizeEnabled: settings.minimizeEnabled !== false,
        fourFingerSwipeDownMinimizeAllEnabled:
            settings.fourFingerSwipeDownMinimizeAllEnabled !== false,
        swipeDownAction: ['minimize', 'close', 'choose'].includes(settings.swipeDownAction)
            ? settings.swipeDownAction
            : 'minimize',
        swipeDownThreshold: finiteNumber(settings.swipeDownThreshold, 0.02, 0.30)
            ? settings.swipeDownThreshold
            : 0.15,
        centerEnabled: settings.centerEnabled !== false,
        monitorMoveEnabled: settings.monitorMoveEnabled === true,
        previewDesktopDestination: settings.previewDesktopDestination === true,
        createDesktopOnOverflow: settings.createDesktopOnOverflow === true,
        fiveFingerEnabled: settings.fiveFingerEnabled === true,
        resizeHorizontalEnabled: settings.resizeHorizontalEnabled === true,
        resizeVerticalEnabled: settings.resizeVerticalEnabled === true,
        livePreview: settings.livePreview === true,
        moveCursor: settings.moveCursor === true,
        appSwitchOnHold: settings.appSwitchOnHold === true,
        monitorMoveModifier: normalizeModifier(settings.monitorMoveModifier, 'alt'),
        appCompatibilityMode: settings.appCompatibilityMode === 'requireModifier'
            ? 'requireModifier'
            : 'exclude',
        appCompatibilityModifier: normalizeModifier(
            settings.appCompatibilityModifier, 'ctrl'),
        appCompatibilityProcessNames: Array.isArray(settings.appCompatibilityProcessNames)
            ? settings.appCompatibilityProcessNames
                .filter(value => typeof value === 'string')
                .slice(0, 256)
                .map(value => value.trim().toLowerCase())
                .filter(Boolean)
            : [],
        gridSpacing: Number.isFinite(settings.gridSpacing)
            ? Math.max(0, Math.min(10, Math.round(settings.gridSpacing)))
            : 0,
        overlayColor: /^#[0-9a-fA-F]{6}$/.test(settings.overlayColor ?? '')
            ? settings.overlayColor.toUpperCase()
            : '#0A84FF',
        hudBackground: ['dark', 'light', 'system'].includes(settings.hudBackground)
            ? settings.hudBackground
            : 'dark',
        hudSize: settings.hudSize === 'large' ? 'large' : 'normal',
        hudFadeOutSeconds: finiteNumber(settings.hudFadeOutSeconds, 0.1, 1.5)
            ? settings.hudFadeOutSeconds
            : 0.36,
    };
}

function normalizeModifier(value, fallback) {
    return ['shift', 'ctrl', 'alt'].includes(value) ? value : fallback;
}
