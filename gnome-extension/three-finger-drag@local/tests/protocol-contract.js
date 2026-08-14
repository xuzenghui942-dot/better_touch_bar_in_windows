import GLib from 'gi://GLib';

import {
    COMMIT_KINDS,
    UPDATE_KINDS,
    makeCapabilities,
    normalizeSettings,
    parseConfig,
    parseConfigEnvelope,
    parseEvent,
    resolveConfiguration,
    sequenceToNumber,
    validateSessionId,
} from '../protocol.js';
import {
    adaptiveSnapDuration,
    snapPreviewRect,
    snapZoneFraction,
    zoneRect,
} from '../geometry.js';

const [modulePath] = GLib.filename_from_uri(import.meta.url);
const fixturePath = GLib.build_filenamev([
    GLib.path_get_dirname(modulePath),
    'fixtures.json',
]);
const [ok, fixtureBytes] = GLib.file_get_contents(fixturePath);
assert(ok, 'fixture must be readable');
const fixtures = JSON.parse(new TextDecoder().decode(fixtureBytes));

const config = parseConfig(JSON.stringify(fixtures.config));
const disabled = parseConfigEnvelope(
    '{"version":1,"advancedEnabled":false,"settings":{}}');
assert(disabled.advancedEnabled === false, 'Configure must accept explicit disable');
const disabledConfiguration = resolveConfiguration(disabled, ':1.42', ':1.42');
assert(disabledConfiguration.sender === ':1.42' &&
    disabledConfiguration.settings === null,
'Configure(false) must retain the unique sender while clearing settings');
expectFailure(() => resolveConfiguration(disabled, ':1.42', ':1.99'));
const initiallyDisabled = resolveConfiguration(disabled, null, ':1.42');
assert(initiallyDisabled.sender === ':1.42',
    'an initially disabled configuration must still claim its unique sender');
const settings = normalizeSettings(config.settings);
assert(settings.gridSpacing === 4, 'grid spacing must be preserved');
assert(settings.overlayColor === '#0A84FF', 'valid overlay color must be preserved');
assert(validateSessionId('gesture-1:worker.2'), 'valid session id rejected');
assert(!validateSessionId('../bad session'), 'unsafe session id accepted');
assert(sequenceToNumber(1n) === 1, 'uint64 sequence conversion failed');

const capabilities = makeCapabilities('fixture-generation');
assert(capabilities.protocolVersion === 1 && capabilities.generation,
    'versioned capability handshake is incomplete');
assert(capabilities.rebaselineFeedback === true,
    'down-chooser reversal must support recognizer rebaselining');
assert(capabilities.contactArbitration.twoFinger === true &&
    capabilities.contactArbitration.fiveFinger === false &&
    capabilities.contactArbitration.threeFinger === true &&
    capabilities.contactArbitration.fourFinger === false,
    'contact arbitration capability mismatch');
assert(!Object.hasOwn(capabilities, 'nativeGestureArbitration'),
    'the broker must emit only the canonical arbitration field');
assert(!Object.hasOwn(capabilities.actions, 'existingWorkspace') &&
    !Object.hasOwn(capabilities.actions, 'existingMonitor'),
    'the broker must emit only canonical workspace and monitor action fields');
assert(!Object.hasOwn(capabilities.actions, 'snapThirds'),
    'removed three-column snapping must not be advertised');
for (const removed of ['gridModifierEnabled', 'gridModifier', 'sensitivity'])
    assert(!Object.hasOwn(settings, removed),
        `removed three-column setting remains normalized: ${removed}`);
for (const action of [
    'snap', 'minimize', 'minimizeAll', 'workspace', 'monitorMove',
    'axisResize', 'pinch', 'close', 'dynamicWorkspace', 'hud',
    'animation', 'livePreview', 'moveCursor', 'appSwitch',
    'snapPreview', 'adaptiveSnapAnimation',
])
    assert(capabilities.actions[action] === true,
        `proxied two-finger action ${action} must be available`);
assert(capabilities.actions.freeMove === false &&
    capabilities.actions.freeResize === false,
    'five-finger movement must remain disabled');
assert(settings.animateSnaps === true && settings.snapAnimationSeconds === 0.22,
    'snap animation settings were not normalized');
assert(settings.moveCursor === false && settings.livePreview === false,
    'optional cursor and live-preview settings must stay opt-in');
assert(settings.fourFingerSwipeDownMinimizeAllEnabled === true,
    'four-finger-down minimize-all must default on inside advanced mode');

parseEvent(JSON.stringify(fixtures.update), UPDATE_KINDS);
parseEvent(JSON.stringify(fixtures.commit), COMMIT_KINDS);
expectFailure(() => parseConfig('{"version":1,"advancedEnabled":false,"settings":{}}'));
expectFailure(() => parseEvent(
    '{"version":1,"kind":"completed","direction":"left","windowId":7}',
    COMMIT_KINDS));
expectFailure(() => parseEvent(
    '{"version":1,"kind":"freeMoveDelta","dx":99,"dy":0,"scale":1}',
    UPDATE_KINDS));

const left = zoneRect({x: 0, y: 0, width: 101, height: 80}, 'leftHalf', 0);
const right = zoneRect({x: 0, y: 0, width: 101, height: 80}, 'rightHalf', 0);
assert(left.width === 50 && right.x === 50 && right.width === 51,
    'odd-pixel remainder must be assigned to trailing zone');
const scaledWork = {x: 1536, y: 24, width: 1707, height: 933};
const scaledTopLeft = zoneRect(scaledWork, 'topLeft', 0);
const scaledTopRight = zoneRect(scaledWork, 'topRight', 0);
const scaledBottomRight = zoneRect(scaledWork, 'bottomRight', 0);
assert(scaledTopLeft.x === 1536 && scaledTopLeft.width === 853 &&
    scaledTopRight.x === 2389 && scaledTopRight.width === 854 &&
    scaledBottomRight.y === 490 && scaledBottomRight.height === 467,
'fractional-scale logical work areas must tile without gaps or dropped pixels');

const expectedFractions = {
    leftHalf: {x: 0, y: 0, width: 0.5, height: 1},
    rightHalf: {x: 0.5, y: 0, width: 0.5, height: 1},
    topLeft: {x: 0, y: 0, width: 0.5, height: 0.5},
    topRight: {x: 0.5, y: 0, width: 0.5, height: 0.5},
    bottomLeft: {x: 0, y: 0.5, width: 0.5, height: 0.5},
    bottomRight: {x: 0.5, y: 0.5, width: 0.5, height: 0.5},
};
for (const [zone, expected] of Object.entries(expectedFractions))
    assert(JSON.stringify(snapZoneFraction(zone)) === JSON.stringify(expected),
        `${zone} preview fraction is incorrect`);
assert(snapZoneFraction('maximize') === null && snapZoneFraction('minimize') === null,
    'preview fractions must be limited to half and quarter zones');

const monitor = {x: 100, y: 50, width: 800, height: 600};
assert(JSON.stringify(snapPreviewRect(
    {x: 200, y: 120}, monitor, {width: 68, height: 44})) ===
    JSON.stringify({x: 218, y: 138, width: 68, height: 44}),
'preview must normally follow the pointer at the configured gap');
assert(JSON.stringify(snapPreviewRect(
    {x: 892, y: 642}, monitor, {width: 68, height: 44})) ===
    JSON.stringify({x: 806, y: 580, width: 68, height: 44}),
'preview must flip left/up before crossing the active monitor edge');
assert(JSON.stringify(snapPreviewRect(
    {x: -500, y: -500}, monitor, {width: 68, height: 44})) ===
    JSON.stringify({x: 108, y: 58, width: 68, height: 44}),
'preview must clamp to an eight-pixel active-monitor margin');

const work = {x: 0, y: 0, width: 1920, height: 1048};
const subtleDuration = adaptiveSnapDuration(
    {x: 0, y: 0, width: 960, height: 1048},
    {x: 0, y: 0, width: 960, height: 1048}, work);
const halfDuration = adaptiveSnapDuration(
    {x: 100, y: 100, width: 900, height: 700},
    {x: 960, y: 0, width: 960, height: 1048}, work);
const extremeDuration = adaptiveSnapDuration(
    {x: -100000, y: -100000, width: 1, height: 1},
    {x: 0, y: 0, width: 1920, height: 1048}, work);
assert(subtleDuration === 180 && halfDuration > subtleDuration && halfDuration <= 260 &&
    extremeDuration === 260,
'adaptive snap duration must be ordered and clamped to 180-260ms');

print('GNOME broker protocol/geometry contract: ok');

function expectFailure(callback) {
    try {
        callback();
    } catch (_error) {
        return;
    }
    throw new Error('expected validation failure');
}

function assert(condition, message) {
    if (!condition)
        throw new Error(message);
}
