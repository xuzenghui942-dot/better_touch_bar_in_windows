import Clutter from 'gi://Clutter';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';

/** GNOME Shell rendering for the Windows Swoosh destination HUD. */
export class GestureHud {
    constructor() {
        this._settings = {
            hudBackground: 'dark',
            hudSize: 'normal',
            hudFadeOutSeconds: 0.36,
        };
        this._label = new St.Label({
            text: '',
            x_align: Clutter.ActorAlign.CENTER,
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._actor = new St.Bin({
            child: this._label,
            reactive: false,
            can_focus: false,
            visible: false,
            opacity: 0,
        });
        const actor = this._actor;
        this._actorDestroyId = actor.connect('destroy', () => {
            if (this._actor === actor) {
                this._actor = null;
                this._label = null;
            }
            this._actorDestroyId = 0;
        });
        Main.layoutManager.addTopChrome(this._actor, {
            affectsStruts: false,
            trackFullscreen: true,
        });
    }

    configure(settings) {
        this._settings = {...this._settings, ...settings};
    }

    skipNextWindowEffect(actor) {
        Main.wm?.skipNextEffect?.(actor);
    }

    showBadge(label, color) {
        const [pointerX, pointerY] = global.get_pointer();
        const scale = this._settings.hudSize === 'large' ? 1 : 0.78;
        const width = Math.round(210 * scale);
        const height = Math.round(64 * scale);
        this._showAt({
            x: Math.round(pointerX + 18),
            y: Math.round(pointerY + 18),
            width,
            height,
        }, label, color);
    }

    showChooser(canChoose, closeSelected, color) {
        const label = canChoose
            ? (closeSelected ? '− Minimize   [Close ×]' : '[− Minimize]   Close ×')
            : 'Release: close window';
        this.showBadge(label, color);
    }

    showStrip(items, selected, title, color) {
        const safeItems = items.slice(0, 10).map((item, index) =>
            index === selected ? `[ ${item} ]` : String(item));
        const text = `${title}\n${safeItems.join('   ')}`;
        const [pointerX, pointerY] = global.get_pointer();
        const large = this._settings.hudSize === 'large';
        this._showAt({
            x: Math.round(pointerX - (large ? 260 : 210)),
            y: Math.round(pointerY + 22),
            width: large ? 520 : 420,
            height: large ? 104 : 84,
        }, text, color);
    }

    _showAt(rect, label, color) {
        const actor = this._actor;
        const actorLabel = this._label;
        if (!actor || !actorLabel)
            return;
        try {
            actor.remove_all_transitions();
            actorLabel.text = label ?? '';
            actor.set_position(rect.x, rect.y);
            actor.set_size(rect.width, rect.height);
            actor.set_style(this._style(color));
            actor.opacity = 255;
            actor.show();
        } catch (error) {
            this._forgetDisposedActor(actor, error);
        }
    }

    _style(color) {
        const light = this._settings.hudBackground === 'light';
        const systemLight = this._settings.hudBackground === 'system' &&
            St.Settings.get().color_scheme === St.SystemColorScheme.PREFER_LIGHT;
        const useLight = light || systemLight;
        const background = useLight
            ? 'rgba(248,248,250,0.90)'
            : 'rgba(24,24,28,0.86)';
        const foreground = useLight ? '#111114' : '#FFFFFF';
        const accent = /^#[0-9a-fA-F]{6}$/.test(color ?? '') ? color : '#0A84FF';
        const fontSize = this._settings.hudSize === 'large' ? 17 : 14;
        return `background-color: ${background}; color: ${foreground}; ` +
            `border: 3px solid ${accent}; border-radius: 14px; ` +
            `padding: 10px 14px; font-size: ${fontSize}px; font-weight: 600;`;
    }

    hide(immediate = false) {
        const actor = this._actor;
        if (!actor)
            return;
        try {
            if (!actor.visible)
                return;
            actor.remove_all_transitions();
            if (immediate) {
                actor.hide();
                actor.opacity = 0;
                return;
            }
            const duration = Math.round(this._settings.hudFadeOutSeconds * 1000);
            actor.ease({
                opacity: 0,
                duration,
                mode: Clutter.AnimationMode.EASE_OUT_QUAD,
                onComplete: () => {
                    if (this._actor !== actor)
                        return;
                    try {
                        actor.hide();
                    } catch (error) {
                        this._forgetDisposedActor(actor, error);
                    }
                },
            });
        } catch (error) {
            this._forgetDisposedActor(actor, error);
        }
    }

    destroy() {
        const actor = this._actor;
        if (!actor)
            return;
        this._actor = null;
        this._label = null;
        const destroyId = this._actorDestroyId;
        this._actorDestroyId = 0;
        try {
            if (destroyId)
                actor.disconnect(destroyId);
            actor.remove_all_transitions();
            actor.hide();
            actor.opacity = 0;
            actor.destroy();
        } catch (error) {
            console.warn(`three-finger-drag HUD cleanup skipped: ${error.message}`);
        }
    }

    _forgetDisposedActor(actor, error) {
        if (this._actor === actor) {
            this._actor = null;
            this._label = null;
            this._actorDestroyId = 0;
        }
        console.warn(`three-finger-drag HUD actor unavailable: ${error.message}`);
    }
}
