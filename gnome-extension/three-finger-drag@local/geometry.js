export const MIN_WINDOW_WIDTH = 260;
export const MIN_WINDOW_HEIGHT = 180;

export function copyRect(rect) {
    return {x: rect.x, y: rect.y, width: rect.width, height: rect.height};
}

export function zoneRect(work, zone, spacing = 0) {
    const halfWidth = Math.floor(work.width / 2);
    const halfHeight = Math.floor(work.height / 2);
    const thirdWidth = Math.floor(work.width / 3);
    const thirdHeight = Math.floor(work.height / 3);
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
    case 'leftThird':
        rect = sized(work.x, work.y, thirdWidth, work.height);
        break;
    case 'centerThird':
        rect = sized(work.x + thirdWidth, work.y, thirdWidth, work.height);
        break;
    case 'rightThird':
        rect = sized(work.x + 2 * thirdWidth, work.y,
            work.width - 2 * thirdWidth, work.height);
        break;
    case 'leftTwoThird':
        rect = sized(work.x, work.y, 2 * thirdWidth, work.height);
        break;
    case 'rightTwoThird':
        rect = sized(work.x + thirdWidth, work.y,
            work.width - thirdWidth, work.height);
        break;
    case 'topThird':
        rect = sized(work.x, work.y, work.width, thirdHeight);
        break;
    case 'centerRowThird':
        rect = sized(work.x, work.y + thirdHeight, work.width, thirdHeight);
        break;
    case 'bottomThird':
        rect = sized(work.x, work.y + 2 * thirdHeight,
            work.width, work.height - 2 * thirdHeight);
        break;
    case 'topTwoThird':
        rect = sized(work.x, work.y, work.width, 2 * thirdHeight);
        break;
    case 'bottomTwoThird':
        rect = sized(work.x, work.y + thirdHeight,
            work.width, work.height - thirdHeight);
        break;
    case 'thirdTopLeft':
        rect = sized(work.x, work.y, thirdWidth, thirdHeight);
        break;
    case 'thirdTopRight':
        rect = sized(work.x + 2 * thirdWidth, work.y,
            work.width - 2 * thirdWidth, thirdHeight);
        break;
    case 'thirdBottomLeft':
        rect = sized(work.x, work.y + 2 * thirdHeight,
            thirdWidth, work.height - 2 * thirdHeight);
        break;
    case 'thirdBottomRight':
        rect = sized(work.x + 2 * thirdWidth, work.y + 2 * thirdHeight,
            work.width - 2 * thirdWidth, work.height - 2 * thirdHeight);
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

export function thirdsZone(dx, dy, sensitivity = 0.1) {
    const ax = Math.abs(dx);
    const ay = Math.abs(dy);
    const maximum = Math.max(ax, ay);
    if (maximum < 0.03)
        return 'none';
    const diagonalRatio = 0.9 - 0.5 * sensitivity;
    if (Math.min(ax, ay) >= 0.06 && Math.min(ax, ay) / maximum >= diagonalRatio) {
        if (dx < 0 && dy < 0)
            return 'thirdTopLeft';
        if (dx >= 0 && dy < 0)
            return 'thirdTopRight';
        if (dx < 0)
            return 'thirdBottomLeft';
        return 'thirdBottomRight';
    }
    if (ax >= ay) {
        if (ax < 0.06)
            return 'centerThird';
        if (dx < 0)
            return ax < 0.13 ? 'leftTwoThird' : 'leftThird';
        return ax < 0.13 ? 'rightTwoThird' : 'rightThird';
    }
    if (ay < 0.06)
        return 'centerRowThird';
    if (dy < 0)
        return ay < 0.13 ? 'topTwoThird' : 'topThird';
    return ay < 0.13 ? 'bottomTwoThird' : 'bottomThird';
}

function sized(x, y, width, height) {
    return {x, y, width, height};
}
