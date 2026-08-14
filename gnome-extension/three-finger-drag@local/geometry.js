export const MIN_WINDOW_WIDTH = 260;
export const MIN_WINDOW_HEIGHT = 180;

export function copyRect(rect) {
    return {x: rect.x, y: rect.y, width: rect.width, height: rect.height};
}

export function zoneRect(work, zone, spacing = 0) {
    const halfWidth = Math.floor(work.width / 2);
    const halfHeight = Math.floor(work.height / 2);
    let rect;

    switch (zone) {
    case 'leftHalf':
        rect = sized(work.x, work.y, halfWidth, work.height);
        break;
    case 'rightHalf':
        rect = sized(work.x + halfWidth, work.y,
            work.width - halfWidth, work.height);
        break;
    case 'topHalf':
        rect = sized(work.x, work.y, work.width, halfHeight);
        break;
    case 'bottomHalf':
        rect = sized(work.x, work.y + halfHeight,
            work.width, work.height - halfHeight);
        break;
    case 'topLeft':
        rect = sized(work.x, work.y, halfWidth, halfHeight);
        break;
    case 'topRight':
        rect = sized(work.x + halfWidth, work.y,
            work.width - halfWidth, halfHeight);
        break;
    case 'bottomLeft':
        rect = sized(work.x, work.y + halfHeight,
            halfWidth, work.height - halfHeight);
        break;
    case 'bottomRight':
        rect = sized(work.x + halfWidth, work.y + halfHeight,
            work.width - halfWidth, work.height - halfHeight);
        break;
    case 'center':
        rect = sized(work.x + Math.floor(work.width / 6),
            work.y + Math.floor(work.height / 6),
            Math.floor(work.width * 2 / 3), Math.floor(work.height * 2 / 3));
        break;
    default:
        rect = copyRect(work);
        break;
    }

    const gap = Math.max(0, Math.min(10, Math.round(spacing)));
    if (gap > 0 && rect.width > 4 * gap && rect.height > 4 * gap) {
        rect.x += gap;
        rect.y += gap;
        rect.width -= 2 * gap;
        rect.height -= 2 * gap;
    }
    return rect;
}

export function clampRect(rect, work) {
    const width = Math.max(Math.min(rect.width, work.width),
        Math.min(MIN_WINDOW_WIDTH, work.width));
    const height = Math.max(Math.min(rect.height, work.height),
        Math.min(MIN_WINDOW_HEIGHT, work.height));
    return {
        x: Math.max(work.x, Math.min(rect.x, work.x + work.width - width)),
        y: Math.max(work.y, Math.min(rect.y, work.y + work.height - height)),
        width,
        height,
    };
}

export function standardZone(direction, settings) {
    const zones = {
        left: settings.halvesEnabled ? 'leftHalf' : 'none',
        right: settings.halvesEnabled ? 'rightHalf' : 'none',
        up: settings.maximizeEnabled ? 'maximize' : 'none',
        down: settings.minimizeEnabled ? 'minimize' : 'none',
        upLeft: settings.quartersEnabled ? 'topLeft' : 'none',
        upRight: settings.quartersEnabled ? 'topRight' : 'none',
        downLeft: settings.quartersEnabled ? 'bottomLeft' : 'none',
        downRight: settings.quartersEnabled ? 'bottomRight' : 'none',
    };
    return zones[direction] ?? 'none';
}

export function snapZoneFraction(zone) {
    const fractions = {
        leftHalf: {x: 0, y: 0, width: 0.5, height: 1},
        rightHalf: {x: 0.5, y: 0, width: 0.5, height: 1},
        topLeft: {x: 0, y: 0, width: 0.5, height: 0.5},
        topRight: {x: 0.5, y: 0, width: 0.5, height: 0.5},
        bottomLeft: {x: 0, y: 0.5, width: 0.5, height: 0.5},
        bottomRight: {x: 0.5, y: 0.5, width: 0.5, height: 0.5},
    };
    return fractions[zone] ?? null;
}

export function isHalfOrQuarterZone(zone) {
    return snapZoneFraction(zone) !== null;
}

export function snapPreviewRect(pointer, monitor, size, gap = 18, margin = 8) {
    const width = Math.max(1, Math.round(size.width));
    const height = Math.max(1, Math.round(size.height));
    const safeGap = Math.max(0, Math.round(gap));
    const safeMargin = Math.max(0, Math.round(margin));
    const minimumX = Math.round(monitor.x + safeMargin);
    const minimumY = Math.round(monitor.y + safeMargin);
    const maximumX = Math.max(minimumX,
        Math.round(monitor.x + monitor.width - safeMargin - width));
    const maximumY = Math.max(minimumY,
        Math.round(monitor.y + monitor.height - safeMargin - height));

    let x = Math.round(pointer.x + safeGap);
    let y = Math.round(pointer.y + safeGap);
    if (x + width > monitor.x + monitor.width - safeMargin)
        x = Math.round(pointer.x - safeGap - width);
    if (y + height > monitor.y + monitor.height - safeMargin)
        y = Math.round(pointer.y - safeGap - height);

    return {
        x: Math.max(minimumX, Math.min(maximumX, x)),
        y: Math.max(minimumY, Math.min(maximumY, y)),
        width,
        height,
    };
}

export function adaptiveSnapDuration(source, target, work) {
    const workWidth = Math.max(1, work.width);
    const workHeight = Math.max(1, work.height);
    const sourceWidth = Math.max(1, source.width);
    const sourceHeight = Math.max(1, source.height);
    const targetWidth = Math.max(1, target.width);
    const targetHeight = Math.max(1, target.height);
    const sourceCenterX = source.x + sourceWidth / 2;
    const sourceCenterY = source.y + sourceHeight / 2;
    const targetCenterX = target.x + targetWidth / 2;
    const targetCenterY = target.y + targetHeight / 2;
    const move = Math.hypot(
        (sourceCenterX - targetCenterX) / workWidth,
        (sourceCenterY - targetCenterY) / workHeight);
    const resize = Math.max(
        Math.abs(Math.log(targetWidth / sourceWidth)),
        Math.abs(Math.log(targetHeight / sourceHeight)));
    const intensity = Math.max(0, Math.min(1, Math.max(move, resize * 0.5)));
    return Math.round(180 + 80 * intensity);
}

function sized(x, y, width, height) {
    return {x, y, width, height};
}
