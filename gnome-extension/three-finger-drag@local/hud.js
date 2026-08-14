import Clutter from 'gi://Clutter';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {snapPreviewRect, snapZoneFraction} from './geometry.js';

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

        this._previewFill = new St.Widget({reactive: false, can_focus: false});
        this._previewVertical = new St.Widget({reactive: false, can_focus: false});
        this._previewHorizontal = new St.Widget({reactive: false, can_focus: false});
        this._previewActor = new St.Widget({
            reactive: false,
            can_focus: false,
            visible: false,
            opacity: 0,
            clip_to_allocation: true,
        });
        this._previewActor.add_child(this._previewFill);
        this._previewActor.add_child(this._previewVertical);
        this._previewActor.add_child(this._previewHorizontal);
        const previewActor = this._previewActor;
        this._previewDestroyId = previewActor.connect('destroy', () => {
            if (this._previewActor === previewActor) {
                this._previewActor = null;
                this._previewFill = null;
                this._previewVertical = null;
                this._previewHorizontal = null;
            }
            this._previewDestroyId = 0;
        });
        Main.layoutManager.addTopChrome(previewActor, {
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

    showSnapPreview(zone, color, monitor) {
        const fraction = snapZoneFraction(zone);
        const actor = this._previewActor;
        const fill = this._previewFill;
        const vertical = this._previewVertical;
        const horizontal = this._previewHorizontal;
        if (!fraction || !actor || !fill || !vertical || !horizontal || !monitor)
            return;

        const large = this._settings.hudSize === 'large';
        const size = large
            ? {width: 96, height: 62}
            : {width: 68, height: 44};
        const [pointerX, pointerY] = global.get_pointer();
        const rect = snapPreviewRect(
            {x: pointerX, y: pointerY}, monitor, size);
        const inset = large ? 6 : 5;
        const innerWidth = Math.max(1, rect.width - inset * 2);
        const innerHeight = Math.max(1, rect.height - inset * 2);
        const fillX = inset + Math.floor(innerWidth * fraction.x);
        const fillY = inset + Math.floor(innerHeight * fraction.y);
        const fillRight = inset + (fraction.x + fraction.width >= 1
            ? innerWidth
            : Math.floor(innerWidth * (fraction.x + fraction.width)));
        const fillBottom = inset + (fraction.y + fraction.height >= 1
            ? innerHeight
            : Math.floor(innerHeight * (fraction.y + fraction.height)));
        const middleX = inset + Math.floor(innerWidth / 2);
        const middleY = inset + Math.floor(innerHeight / 2);
        const palette = this._palette(color);

        try {
            this._hideActor(this._actor, true, 0);
            actor.remove_all_transitions();
            actor.set_position(rect.x, rect.y);
            actor.set_size(rect.width, rect.height);
            actor.set_style(`background-color: ${palette.background}; ` +
                `border: 2px solid ${palette.accent}; ` +
                `border-radius: ${large ? 13 : 10}px;`);
            fill.set_position(fillX, fillY);
            fill.set_size(Math.max(1, fillRight - fillX), Math.max(1, fillBottom - fillY));
            fill.set_style(`background-color: ${palette.accent}; border-radius: 4px;`);
            fill.opacity = 184;
            vertical.set_position(middleX, inset);
            vertical.set_size(1, innerHeight);
            vertical.set_style(`background-color: ${palette.divider};`);
            horizontal.set_position(inset, middleY);
            horizontal.set_size(innerWidth, 1);
            horizontal.set_style(`background-color: ${palette.divider};`);
            actor.opacity = 255;
            actor.show();
        } catch (error) {
            this._forgetDisposedActor(actor, error);
        }
    }

    _showAt(rect, label, color) {
        const actor = this._actor;
        const actorLabel = this._label;
        if (!actor || !actorLabel)
            return;
        try {
            this.hideSnapPreview(true);
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
        const {background, foreground, accent} = this._palette(color);
        const fontSize = this._settings.hudSize === 'large' ? 17 : 14;
        return `background-color: ${background}; color: ${foreground}; ` +
            `border: 3px solid ${accent}; border-radius: 14px; ` +
            `padding: 10px 14px; font-size: ${fontSize}px; font-weight: 600;`;
    }

    _palette(color) {
        const light = this._settings.hudBackground === 'light';
        const systemLight = this._settings.hudBackground === 'system' &&
            St.Settings.get().color_scheme === St.SystemColorScheme.PREFER_LIGHT;
        const useLight = light || systemLight;
        return {
            background: useLight ? 'rgba(248,248,250,0.94)' : 'rgba(24,24,28,0.90)',
            foreground: useLight ? '#111114' : '#FFFFFF',
            divider: useLight ? 'rgba(17,17,20,0.36)' : 'rgba(255,255,255,0.40)',
            accent: /^#[0-9a-fA-F]{6}$/.test(color ?? '') ? color : '#0A84FF',
        };
    }

    hide(immediate = false) {
        const duration = Math.round(this._settings.hudFadeOutSeconds * 1000);
        this._hideActor(this._actor, immediate, duration);
        this.hideSnapPreview(immediate);
    }

    hideSnapPreview(immediate = false) {
        this._hideActor(this._previewActor, immediate, 100);
    }

    _hideActor(actor, immediate, duration) {
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
            actor.ease({
                opacity: 0,
                duration,
                mode: Clutter.AnimationMode.EASE_OUT_QUAD,
                onComplete: () => {
                    if (this._actor !== actor && this._previewActor !== actor)
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
        const previewActor = this._previewActor;
        this._actor = null;
        this._label = null;
        this._previewActor = null;
        this._previewFill = null;
        this._previewVertical = null;
        this._previewHorizontal = null;
        const actors = [
            [actor, this._actorDestroyId],
            [previewActor, this._previewDestroyId],
        ];
        this._actorDestroyId = 0;
        this._previewDestroyId = 0;
        for (const [ownedActor, destroyId] of actors) {
            if (!ownedActor)
                continue;
            try {
                if (destroyId)
                    ownedActor.disconnect(destroyId);
                ownedActor.remove_all_transitions();
                ownedActor.hide();
                ownedActor.opacity = 0;
                ownedActor.destroy();
            } catch (error) {
                console.warn(`three-finger-drag HUD cleanup skipped: ${error.message}`);
            }
        }
    }

    _forgetDisposedActor(actor, error) {
        if (this._actor === actor) {
            this._actor = null;
            this._label = null;
            this._actorDestroyId = 0;
        }
        if (this._previewActor === actor) {
            this._previewActor = null;
            this._previewFill = null;
            this._previewVertical = null;
            this._previewHorizontal = null;
            this._previewDestroyId = 0;
        }
        console.warn(`three-finger-drag HUD actor unavailable: ${error.message}`);
    }
}
