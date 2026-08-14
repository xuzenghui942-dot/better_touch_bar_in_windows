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
import {thirdsZone, zoneRect} from '../geometry.js';

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
for (const action of [
    'snap', 'minimize', 'minimizeAll', 'workspace', 'monitorMove',
    'axisResize', 'pinch', 'close', 'dynamicWorkspace', 'hud',
    'animation', 'livePreview', 'moveCursor', 'appSwitch',
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
assert(thirdsZone(-0.15, 0, 0.1) === 'leftThird', 'thirds mapping failed');

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
