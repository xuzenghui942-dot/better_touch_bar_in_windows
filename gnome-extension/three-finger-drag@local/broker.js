import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

import {GestureHud} from './hud.js';
import {FourFingerDownGestureSet} from './fourFingerDown.js';
import {
    BUS_NAME,
    OBJECT_PATH,
    COMMIT_KINDS,
    ProbeTokenStore,
    UPDATE_KINDS,
    makeCapabilities,
    parseConfig,
    parseConfigEnvelope,
    parseEvent,
    resolveConfiguration,
    sequenceToNumber,
    validateSessionId,
} from './protocol.js';
import {WindowBackend} from './windowBackend.js';

const BEGIN_KINDS = new Set(['began']);
const IDLE_TIMEOUT_US = 2_000_000;
const MAX_SESSION_US = 12_000_000;

/**
 * GNOME-side window transaction broker. Exclusive two-finger ownership is
 * provided earlier by the Rust evdev/uinput proxy; this object never tries to
 * stop a native Wayland client gesture after delivery.
 */
export class CapabilityBroker {
    constructor(interfaceXml) {
        this._generation = GLib.uuid_string_random();
        this._destroyed = false;
        this._configuredSender = null;
        this._configuredSettings = null;
        this._senderWatchId = 0;
        this._active = null;
        this._probeTokens = new ProbeTokenStore(
            () => GLib.uuid_string_random(), () => GLib.get_monotonic_time());
        this._watchdogId = 0;
        this._hud = null;
        this._backend = null;
        this._fourFingerDown = new FourFingerDownGestureSet();
        this._fourFingerCommitId = 0;

        this._dbusObject = Gio.DBusExportedObject.wrapJSObject(interfaceXml, this);
        this._dbusObject.export(Gio.DBus.session, OBJECT_PATH);
        this._busOwnerId = Gio.DBus.session.own_name(
            BUS_NAME,
            Gio.BusNameOwnerFlags.NONE,
            null,
            () => this._clearConfiguration());
    }

    GetCapabilities() {
        return JSON.stringify(makeCapabilities(this._generation));
    }

    GetModifiers() {
        const modifiers = this._backend?.getModifiers() ?? {monitor: false};
        return [false, modifiers.monitor, false];
    }

    observeFourFingerSwipe(device, phase, fingers, dx, dy) {
        if (this._configuredSettings?.fourFingerSwipeDownMinimizeAllEnabled !== true) {
            this._fourFingerDown.reset();
            return;
        }
        if (!this._fourFingerDown.observe(device, phase, fingers, dx, dy))
            return;

        // The capture handler runs before GNOME's own touchpad handler. Queue
        // the action so the complete native four-finger stream is delivered
        // first; no other direction is intercepted or changed.
        if (this._fourFingerCommitId)
            return;
        this._fourFingerCommitId = GLib.idle_add(GLib.PRIORITY_DEFAULT_IDLE, () => {
            this._fourFingerCommitId = 0;
            if (this._configuredSettings?.fourFingerSwipeDownMinimizeAllEnabled === true) {
                this._cancelActive();
                this._ensureBackend();
                this._backend.minimizeAllOnActiveWorkspace();
            }
            return GLib.SOURCE_REMOVE;
        });
    }

    ConfigureAsync([configJson], invocation) {
        let accepted = false;
        try {
            this._ensureAlive();
            const sender = invocation.get_sender();
            const envelope = parseConfigEnvelope(configJson);
            const resolved = resolveConfiguration(
                envelope, this._configuredSender, sender);
            this._bindSender(resolved.sender);
            this._cancelActive();
            this._probeTokens.clear();
            this._clearFourFingerState();
            this._configuredSettings = resolved.settings;
            if (this._configuredSettings) {
                this._configuredSettings.fiveFingerEnabled = false;
                this._configuredSettings.centerEnabled = false;
                this._ensureBackend();
                this._backend.configure(this._configuredSettings);
            } else {
                this._destroyBackend();
            }
            accepted = true;
        } catch (error) {
            console.warn(`three-finger-drag broker Configure rejected: ${error.message}`);
        }
        invocation.return_value(new GLib.Variant('(b)', [accepted]));
    }

    ProbeTargetAsync(_parameters, invocation) {
        let accepted = false;
        let targetToken = '';
        let detail = 'request rejected';
        try {
            const sender = invocation.get_sender();
            this._requireConfiguredSender(sender);
            if (this._active)
                throw new Error('another gesture transaction is active');
            this._ensureBackend();
            const outcome = this._backend.probeTarget(this._configuredSettings);
            if (!outcome.accepted) {
                detail = outcome.detail;
            } else {
                targetToken = this._probeTokens.issue(sender, outcome.target);
                accepted = true;
                detail = outcome.detail;
            }
        } catch (error) {
            detail = error.message;
        }
        invocation.return_value(new GLib.Variant(
            '(bss)', [accepted, targetToken, detail]));
    }

    BeginAsync([sessionId, sequence, targetToken, configJson, eventJson], invocation) {
        let accepted = false;
        let detail = 'request rejected';
        const activeBeforeRequest = this._active;
        try {
            const sender = invocation.get_sender();
            this._requireConfiguredSender(sender);
            if (this._active)
                throw new Error('another gesture transaction is active');
            if (!validateSessionId(sessionId))
                throw new Error('session id is invalid');
            const number = sequenceToNumber(sequence);
            if (number !== 1)
                throw new Error('Begin sequence must be 1');
            parseConfig(configJson);
            const event = parseEvent(eventJson, BEGIN_KINDS);
            if (event.contacts !== 2)
                throw new Error('only two-finger advanced gestures are supported');
            const probedTarget = this._probeTokens.consume(sender, targetToken);

            this._ensureBackend();
            const outcome = this._backend.begin(
                this._configuredSettings, event, probedTarget);
            if (!outcome.accepted) {
                detail = outcome.detail;
            } else {
                const now = GLib.get_monotonic_time();
                this._active = {
                    sender,
                    sessionId,
                    sequence: number,
                    startedAt: now,
                    touchedAt: now,
                };
                this._ensureWatchdog();
                accepted = true;
                detail = outcome.detail;
            }
        } catch (error) {
            // A rejected caller/token must never cancel somebody else's
            // already-active transaction. If this request started from idle,
            // cancel only possible partial backend state created by begin().
            if (!activeBeforeRequest && !this._active) {
                try {
                    this._backend?.cancel();
                } catch (cleanupError) {
                    console.warn(`three-finger-drag Begin cleanup failed: ${cleanupError.message}`);
                }
            }
            detail = error.message;
        }
        invocation.return_value(new GLib.Variant('(bs)', [accepted, detail]));
    }

    UpdateAsync([sessionId, sequence, eventJson], invocation) {
        const accepted = this._withActiveRequest(
            invocation.get_sender(), sessionId, sequence, false,
            () => this._backend.handleUpdate(parseEvent(eventJson, UPDATE_KINDS)));
        const rebaseline = accepted
            ? (this._backend.takeRebaselineRequest?.() ?? '')
            : '';
        invocation.return_value(new GLib.Variant('(bs)', [accepted, rebaseline]));
    }

    CommitAsync([sessionId, sequence, eventJson], invocation) {
        const accepted = this._withActiveRequest(
            invocation.get_sender(), sessionId, sequence, true,
            () => this._backend.handleCommit(parseEvent(eventJson, COMMIT_KINDS)));
        invocation.return_value(new GLib.Variant('(b)', [accepted]));
    }

    CancelAsync([sessionId, sequence, reason], invocation) {
        const accepted = this._withActiveRequest(
            invocation.get_sender(), sessionId, sequence, true,
            () => {
                if (typeof reason !== 'string' || reason.length > 256)
                    throw new Error('cancel reason is invalid');
                return true;
            }, true);
        invocation.return_value(new GLib.Variant('(b)', [accepted]));
    }

    destroy() {
        if (this._destroyed)
            return;
        this._destroyed = true;
        this._clearFourFingerState();
        this._clearConfiguration();
        if (this._busOwnerId)
            Gio.DBus.session.unown_name(this._busOwnerId);
        this._busOwnerId = 0;
        if (this._dbusObject) {
            this._dbusObject.unexport();
            this._dbusObject.run_dispose();
            this._dbusObject = null;
        }
    }

    _withActiveRequest(sender, sessionId, sequence, terminal, callback, cancel = false) {
        let accepted = false;
        try {
            this._requireActive(sender, sessionId);
            const number = sequenceToNumber(sequence);
            if (number !== this._active.sequence + 1)
                throw new Error('gesture sequence is not strictly increasing');
            this._active.sequence = number;
            this._active.touchedAt = GLib.get_monotonic_time();
            accepted = callback() === true;
        } catch (error) {
            console.warn(`three-finger-drag broker request rejected: ${error.message}`);
            accepted = false;
        }

        if (terminal) {
            try {
                if (cancel || !accepted)
                    this._backend?.cancel();
                else
                    this._backend?.finish();
            } catch (error) {
                console.warn(`three-finger-drag broker terminal cleanup failed: ${error.message}`);
                accepted = false;
            } finally {
                this._dropActive();
            }
        } else if (!accepted) {
            this._cancelActive();
        }
        return accepted;
    }

    _ensureAlive() {
        if (this._destroyed)
            throw new Error('broker is disabled');
    }

    _requireConfiguredSender(sender) {
        this._ensureAlive();
        if (!this._configuredSettings || sender !== this._configuredSender)
            throw new Error('caller does not own an enabled broker configuration');
    }

    _requireActive(sender, sessionId) {
        this._requireConfiguredSender(sender);
        if (!this._active || this._active.sender !== sender ||
            this._active.sessionId !== sessionId)
            throw new Error('gesture session does not belong to caller');
    }

    _bindSender(sender) {
        if (this._configuredSender === sender)
            return;
        if (this._configuredSender !== null)
            throw new Error('another D-Bus owner configured the broker');
        this._configuredSender = sender;
        this._senderWatchId = Gio.DBus.session.watch_name(
            sender, Gio.BusNameWatcherFlags.NONE, null,
            () => this._clearConfiguration());
    }

    _ensureBackend() {
        if (this._backend)
            return;
        this._hud = new GestureHud();
        this._backend = new WindowBackend(this._hud, () => this._dropActive());
    }

    _destroyBackend() {
        const backend = this._backend;
        const hud = this._hud;
        this._backend = null;
        this._hud = null;
        try {
            backend?.destroy();
        } catch (error) {
            console.warn(`three-finger-drag backend cleanup failed: ${error.message}`);
        }
        try {
            hud?.destroy();
        } catch (error) {
            console.warn(`three-finger-drag HUD cleanup failed: ${error.message}`);
        }
    }

    _ensureWatchdog() {
        if (this._watchdogId)
            return;
        this._watchdogId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 250, () => {
            if (!this._active) {
                this._watchdogId = 0;
                return GLib.SOURCE_REMOVE;
            }
            const now = GLib.get_monotonic_time();
            if (now - this._active.touchedAt >= IDLE_TIMEOUT_US ||
                now - this._active.startedAt >= MAX_SESSION_US) {
                this._cancelActive();
                this._watchdogId = 0;
                return GLib.SOURCE_REMOVE;
            }
            return GLib.SOURCE_CONTINUE;
        });
    }

    _dropActive() {
        this._active = null;
        if (this._watchdogId)
            GLib.source_remove(this._watchdogId);
        this._watchdogId = 0;
    }

    _cancelActive() {
        try {
            this._backend?.cancel();
        } catch (error) {
            console.warn(`three-finger-drag active gesture cleanup failed: ${error.message}`);
        } finally {
            this._dropActive();
        }
    }

    _clearConfiguration() {
        this._cancelActive();
        this._probeTokens.clear();
        this._clearFourFingerState();
        this._configuredSettings = null;
        if (this._senderWatchId)
            Gio.DBus.session.unwatch_name(this._senderWatchId);
        this._senderWatchId = 0;
        this._configuredSender = null;
        this._destroyBackend();
    }

    _clearFourFingerState() {
        this._fourFingerDown.reset();
        if (this._fourFingerCommitId)
            GLib.source_remove(this._fourFingerCommitId);
        this._fourFingerCommitId = 0;
    }
}
