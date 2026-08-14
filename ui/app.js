"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

let currentSnapshot = null;
let rendering = false;
let saveTimer = null;
let advancedSaveTimer = null;
let advancedSaveInFlight = false;
let advancedSaveQueued = false;
let advancedDirty = false;
let advancedEditRevision = 0;
let statusRefreshTimer = null;
let advancedAppCatalog = [];
let pendingAdvancedRuntime = null;

const byId = (id) => document.getElementById(id);

document.addEventListener("DOMContentLoaded", () => {
  bindTabs();
  bindGestureControls();
  bindAdvancedControls();
  bindApplicationControls();
  initialize().catch(showError);
});

async function initialize() {
  // Subscribe before the initial snapshot. The broker can complete its D-Bus
  // handshake while the WebView is loading; subscribing second would lose the
  // only availability transition and leave the UI stale indefinitely.
  await listen("touchpad-event", ({ payload }) => handleTouchpadEvent(payload));
  await refreshSnapshot();
}

function bindTabs() {
  document.querySelectorAll("[data-tab]").forEach((button) => {
    button.addEventListener("click", () => {
      const selected = button.dataset.tab;
      document.querySelectorAll("[data-tab]").forEach((candidate) => {
        const active = candidate.dataset.tab === selected;
        candidate.classList.toggle("is-active", active);
        candidate.setAttribute("aria-selected", String(active));
      });
      document.querySelectorAll(".panel").forEach((panel) => {
        const active = panel.id === `panel-${selected}`;
        panel.classList.toggle("is-active", active);
        panel.hidden = !active;
      });
    });
  });
}

function bindGestureControls() {
  [
    "three-finger-enabled",
    "drag-button",
    "allow-release",
    "release-delay",
    "cursor-averaging",
  ].forEach((id) => {
    byId(id).addEventListener("change", () => {
      updateGestureControlState();
      scheduleGestureSave();
    });
  });

  document.querySelectorAll("[data-pair]").forEach((control) => {
    control.addEventListener("input", () => {
      synchronizePair(control);
      enforceThresholdRelationship(control.dataset.pair);
      scheduleGestureSave();
    });
    control.addEventListener("change", scheduleGestureSave);
  });

  byId("device-settings").addEventListener("input", (event) => {
    const control = event.target.closest("[data-device-control]");
    if (!control) return;
    if (control.dataset.sync) synchronizeDevicePair(control);
    updateGestureControlState();
    scheduleGestureSave();
  });
  byId("device-settings").addEventListener("change", (event) => {
    if (event.target.closest("[data-device-control]")) {
      updateGestureControlState();
      scheduleGestureSave();
    }
  });
}

function bindAdvancedControls() {
  document.querySelectorAll("[data-advanced-control]").forEach((control) => {
    control.addEventListener("change", () => {
      markAdvancedDirty();
      updateAdvancedDerivedUi();
      scheduleAdvancedSave();
    });
    if (["number", "range", "color"].includes(control.type) || control.tagName === "TEXTAREA") {
      control.addEventListener("input", () => {
        markAdvancedDirty();
        updateAdvancedDerivedUi();
        if (control.id === "advanced-app-processes") renderSelectedApps();
        scheduleAdvancedSave();
      });
    }
  });

  byId("advanced-monitor-modifier").addEventListener("change", () => {
    markAdvancedDirty();
    scheduleAdvancedSave();
  });
  document.querySelectorAll("[data-overlay-color]").forEach((button) => {
    button.addEventListener("click", () => {
      byId("advanced-overlay-color").value = button.dataset.overlayColor;
      byId("advanced-overlay-accent").checked = false;
      markAdvancedDirty();
      updateAdvancedDerivedUi();
      scheduleAdvancedSave();
    });
  });
}

function markAdvancedDirty() {
  if (rendering) return;
  advancedDirty = true;
  advancedEditRevision += 1;
}

function bindApplicationControls() {
  byId("open-touchpad-settings").addEventListener("click", () =>
    runOperation("正在打开 GNOME 触摸板设置…", () =>
      invoke("open_touchpad_settings")
    )
  );

  byId("refresh-integration-status").addEventListener("click", async () => {
    const snapshot = await runOperation("正在重新检查 GNOME 集成…", async () => {
      // Keep the focused integration probe, then fetch one authoritative full
      // snapshot so runtime availability cannot be overwritten by stale data.
      await invoke("get_platform_integration");
      return invoke("get_snapshot");
    });
    if (!snapshot) return;
    currentSnapshot = snapshot;
    renderSnapshot(currentSnapshot);
  });

  byId("record-logs").addEventListener("change", async (event) => {
    await runSnapshotToggle(event.target, "set_record_logs", "正在更新日志设置…");
  });

  byId("run-at-startup").addEventListener("change", async (event) => {
    await runSnapshotToggle(event.target, "set_run_at_startup", "正在更新登录启动设置…");
  });

  byId("save-logs").addEventListener("click", async () => {
    const path = await runOperation("正在保存日志…", () => invoke("save_logs"));
    if (path) showStatus(`日志已保存到：${path}`);
  });

  byId("copy-diagnostics").addEventListener("click", async () => {
    const report = await runOperation("正在生成诊断报告…", () =>
      invoke("advanced_diagnostics")
    );
    if (!report) return;
    await copyText(report);
    showStatus("诊断报告已复制。");
  });

  byId("copy-advanced-diagnostics").addEventListener("click", async () => {
    const report = await runOperation("正在生成高级功能诊断…", () =>
      invoke("advanced_diagnostics")
    );
    if (!report) return;
    await copyText(report);
    showStatus("高级功能诊断已复制。");
  });

  byId("restore-advanced-defaults").addEventListener("click", async () => {
    if (!window.confirm("只恢复高级窗口手势的 Linux 安全默认值，确定继续吗？")) return;
    const snapshot = await runOperation("正在恢复高级手势默认值…", () =>
      invoke("restore_advanced_defaults")
    );
    if (!snapshot) return;
    currentSnapshot = snapshot;
    renderSnapshot(currentSnapshot);
  });

  byId("advanced-list-running-apps").addEventListener("click", () =>
    loadAppCatalog("list_running_apps", "正在读取运行中的应用标识…")
  );
  byId("advanced-list-installed-apps").addEventListener("click", () =>
    loadAppCatalog("list_installed_apps", "正在读取已安装应用标识…")
  );
  byId("advanced-search-apps").addEventListener("input", renderAppCatalog);

  document.querySelectorAll("[data-url]").forEach((button) => {
    button.addEventListener("click", () =>
      runOperation("正在打开链接…", () =>
        invoke("open_external", { url: button.dataset.url })
      )
    );
  });

  byId("close-settings").addEventListener("click", () => invoke("close_settings"));
  byId("quit-app").addEventListener("click", async () => {
    if (!window.confirm("退出后三指拖动将立即停止。确定退出吗？")) return;
    await invoke("quit_app");
  });
}

async function refreshSnapshot() {
  const preserveAdvancedControls = advancedDirty || advancedSaveInFlight;
  currentSnapshot = await invoke("get_snapshot");
  if (pendingAdvancedRuntime) {
    currentSnapshot.advanced_runtime = pendingAdvancedRuntime;
    pendingAdvancedRuntime = null;
  }
  renderSnapshot(currentSnapshot, preserveAdvancedControls);
}

function renderSnapshot(snapshot, preserveAdvancedControls = false) {
  rendering = true;
  const settings = snapshot.settings;
  byId("three-finger-enabled").checked = settings.three_finger_drag;
  byId("drag-button").value = settings.drag_button;
  byId("allow-release").checked = settings.allow_release_and_restart;
  byId("release-delay").value = settings.release_delay_ms;
  byId("cursor-averaging").value = settings.cursor_averaging;
  setPairValue("max-move-distance", settings.max_finger_move_distance);
  setPairValue("start-threshold", settings.start_threshold);
  setPairValue("stop-threshold", settings.stop_threshold);
  byId("record-logs").checked = settings.record_logs;
  byId("run-at-startup").checked = settings.run_at_startup === true;
  byId("app-version").textContent = `版本 ${snapshot.version} · ${snapshot.platform}`;

  renderTouchpadStatus(snapshot.touchpad);
  renderDeviceSettings(snapshot);
  renderIntegration(snapshot);
  if (preserveAdvancedControls) {
    applyAdvancedCapabilities(snapshot);
    renderAdvancedStatus(snapshot);
    updateAdvancedDerivedUi();
    renderSelectedApps();
  } else {
    renderAdvanced(snapshot);
  }
  updateGestureControlState();
  rendering = false;
}

function renderIntegration(snapshot) {
  const integration = snapshot.integration;
  byId("session-summary").textContent = integration.session_type || "unknown";
  byId("desktop-summary").textContent = integration.desktop || "unknown";

  const guard = byId("gesture-guard-status");
  if (!integration.is_gnome_wayland) {
    setNotice(
      guard,
      "warning",
      "当前不是经验证的 GNOME Wayland 会话。",
      `会话：${integration.session_type}；桌面：${integration.desktop}。`
    );
  } else if (integration.gesture_guard_active) {
    setNotice(guard, "success", "GNOME 三指系统手势已拦截。", integration.gesture_guard_detail);
  } else if (integration.gesture_guard_enabled) {
    setNotice(
      guard,
      "warning",
      "拦截扩展已启用，但未确认当前 Shell 活动。",
      integration.gesture_guard_detail
    );
  } else if (integration.gesture_guard_installed) {
    setNotice(guard, "warning", "拦截扩展已安装，但未启用。", integration.gesture_guard_detail);
  } else {
    setNotice(guard, "error", "尚未检测到三指手势拦截扩展。", integration.gesture_guard_detail);
  }

  setNotice(
    byId("startup-status"),
    snapshot.startup.severity,
    snapshot.startup.title,
    snapshot.settings.run_at_startup
      ? "已创建当前用户的 GNOME autostart 项；可随时关闭开关回滚。"
      : "默认不持久启动；只在你显式开启时创建用户级 .desktop 项。"
  );
  byId("run-at-startup").disabled =
    integration.autostart_configuration_available !== true;
}

function renderAdvanced(snapshot) {
  const settings = snapshot.advanced ?? {};
  const runtime = snapshot.advanced_runtime ?? {};
  const values = {
    enabled: settings.enabled,
    animateSnaps: settings.animateSnaps,
    snapAnimationSeconds: settings.snapAnimationSeconds,
    maximizeEnabled: settings.maximizeEnabled,
    halvesEnabled: settings.halvesEnabled,
    quartersEnabled: settings.quartersEnabled,
    minimizeEnabled: settings.minimizeEnabled,
    fourFingerSwipeDownMinimizeAllEnabled:
      settings.fourFingerSwipeDownMinimizeAllEnabled,
    swipeDownAction: settings.swipeDownAction,
    swipeDownThreshold: settings.swipeDownThreshold,
    gridSpacing: settings.gridSpacing,
    cancelTimeoutSeconds: settings.cancelTimeoutSeconds,
    appSwitchOnHold: settings.appSwitchOnHold,
    monitorMoveEnabled: settings.monitorMoveEnabled,
    previewDesktopDestination: settings.previewDesktopDestination,
    createDesktopOnOverflow: settings.createDesktopOnOverflow,
    desktopHoldDelaySeconds: settings.desktopHoldDelaySeconds,
    phantomRejection: settings.phantomRejection,
    taskbarIconGesturesEnabled: settings.taskbarIconGesturesEnabled,
    appCompatibilityMode: settings.appCompatibilityMode,
    appCompatibilityModifier: settings.appCompatibilityModifier,
    appCompatibilityProcessNamesText: (settings.appCompatibilityProcessNames ?? []).join("\n"),
    overlayUseAccent: settings.overlayUseAccent,
    overlayColor: settings.overlayColor,
    hudBackground: settings.hudBackground,
    hudSize: settings.hudSize,
    hudFadeOutSeconds: settings.hudFadeOutSeconds,
  };

  document.querySelectorAll("[data-advanced-control]").forEach((control) => {
    const key = control.dataset.advancedControl;
    if (!(key in values) || values[key] === undefined) return;
    if (control.type === "checkbox") control.checked = Boolean(values[key]);
    else control.value = String(values[key]);
  });
  byId("advanced-monitor-modifier").value = settings.monitorMoveEnabled === false
    ? "off"
    : (settings.monitorMoveModifier ?? "alt");

  applyAdvancedCapabilities(snapshot);
  renderAdvancedStatus(snapshot);
  updateAdvancedDerivedUi();
  renderSelectedApps();
}

function renderAdvancedStatus(snapshot) {
  const settings = snapshot.advanced ?? {};
  const runtime = snapshot.advanced_runtime ?? {};

  const runtimeAvailable = runtime.available === true;
  const runtimeEnabled = runtime.enabled === true;
  const master = byId("advanced-enabled");
  // An unavailable broker must not allow a new enable request.  If an older
  // configuration requested enablement, keep the control available so the
  // user can explicitly switch it off while runtime remains fail-closed.
  master.disabled = !runtimeAvailable && !settings.enabled;
  master.title = runtimeAvailable
    ? "broker 能力握手已通过"
    : (runtime.last_error ||
      "双指输入代理或 GNOME broker 尚未完成握手，高级运行态保持关闭");

  byId("advanced-capability-badge").textContent = runtimeAvailable ? "broker 就绪" : "安全关闭";
  byId("advanced-runtime-summary").textContent = runtimeEnabled
    ? "运行中"
    : (runtimeAvailable ? "可用，未启用" : "未握手");
  byId("advanced-lifetime-swooshes").textContent = Number(
    runtime.completed_swooshes ?? 0
  ).toLocaleString("zh-CN");

  const status = byId("advanced-status");
  if (!runtimeAvailable) {
    setNotice(
      status,
      "warning",
      "高级窗口手势保持 fail-closed。",
      runtime.last_error || "GNOME/Wayland broker 尚未完成能力握手；不会执行窗口动作。"
    );
  } else if (runtimeEnabled) {
    setNotice(
      status,
      "success",
      "高级窗口手势已由 broker 启用。",
      `已处理 ${runtime.processed_frames ?? 0} 个输入帧；仅四指向下可执行全部最小化。`
    );
  } else if (settings.enabled) {
    setNotice(
      status,
      "warning",
      "broker 已握手，但运行态尚未确认启用。",
      runtime.last_error || "设置已保存；等待运行态确认，期间不会报告窗口动作成功。"
    );
  } else {
    setNotice(
      status,
      "info",
      "broker 能力已就绪，高级手势当前关闭。",
      "可先配置各项，再显式启用；基础三指拖动保持独立。"
    );
  }

  if (snapshot.integration) {
    snapshot.integration.advanced_window_gestures_available = runtimeAvailable;
  }
}

function applyAdvancedCapabilities(snapshot) {
  const runtime = snapshot.advanced_runtime ?? {};
  const capabilities = runtime.capabilities ?? {};
  const available = runtime.available === true;
  const capabilityRules = {
    "advanced-maximize": ["maximize", "broker 未提供最大化动作"],
    "advanced-halves": ["snapHalves", "broker 未提供半屏吸附"],
    "advanced-quarters": ["snapQuarters", "broker 未提供四分之一吸附"],
    "advanced-minimize": ["minimize", "broker 未提供最小化动作"],
    "advanced-four-finger-minimize-all": ["minimizeAll", "broker 未提供全部最小化动作"],
    "advanced-monitor-move": ["monitorMove", "broker 未提供显示器移动"],
    "advanced-preview-desktop": ["workspace", "broker 未提供工作区移动"],
    "advanced-create-desktop": ["dynamicWorkspace", "broker 未提供动态工作区"],
    "advanced-animate-snaps": ["animation", "broker 未提供吸附动画"],
    "advanced-app-switch": ["appSwitch", "broker 未提供按住后应用切换"],
    "advanced-hud-background": ["hud", "broker 未提供 HUD"],
    "advanced-hud-size": ["hud", "broker 未提供 HUD"],
    "advanced-hud-fade": ["hud", "broker 未提供 HUD"],
  };

  Object.entries(capabilityRules).forEach(([id, [capability, unavailableText]]) => {
    setAdvancedControlAvailability(id, !available || capabilities[capability] === true,
      available ? unavailableText : "broker 尚未握手；可先离线配置，但无法启用运行时");
  });

  const closeSupported = !available || capabilities.close === true;
  const swipeDown = byId("advanced-swipe-down-action");
  swipeDown.querySelectorAll('option[value="close"], option[value="choose"]').forEach((option) => {
    option.disabled = !closeSupported;
  });
  swipeDown.title = closeSupported ? "" : "broker 未提供关闭窗口；保存时将回退为最小化";

  [
    ["advanced-phantom-rejection", "Linux 输入路径尚未接入幽灵触点过滤"],
    ["advanced-taskbar-icon-gestures", "Linux broker 尚未实现 Dock 图标手势"],
    ["advanced-overlay-accent", "Linux broker 尚未读取 GNOME/GTK 强调色"],
  ].forEach(([id, detail]) => setAdvancedControlAvailability(id, false, detail));
}

function setAdvancedControlAvailability(id, supported, detail) {
  const control = byId(id);
  if (!control) return;
  control.disabled = !supported;
  control.title = supported ? "" : detail;
  const row = control.closest(".advanced-row, .advanced-slider-row, .gesture-tile");
  if (row) {
    row.classList.toggle("is-unavailable", !supported);
    row.title = supported ? "" : detail;
  }
  if (id === "advanced-monitor-move") {
    const selector = byId("advanced-monitor-modifier");
    selector.disabled = !supported;
    selector.title = supported ? "" : detail;
  }
}

function updateAdvancedDerivedUi() {
  document.querySelectorAll("[data-output-for]").forEach((output) => {
    const control = byId(output.dataset.outputFor);
    if (!control) return;
    const value = finiteNumber(control.value, 0);
    if (control.id === "advanced-grid-spacing") {
      output.textContent = `${Math.round(value)} px`;
    } else if (control.id === "advanced-swipe-down-threshold") {
      output.textContent = value.toFixed(2);
    } else {
      output.textContent = `${value.toFixed(2)} 秒`;
    }
  });

  byId("advanced-app-modifier-row").hidden =
    byId("advanced-app-compat-mode").value !== "requireModifier";
  const selectedColor = byId("advanced-overlay-color").value.toUpperCase();
  document.querySelectorAll("[data-overlay-color]").forEach((button) => {
    button.classList.toggle("is-selected", button.dataset.overlayColor === selectedColor);
  });
}

async function loadAppCatalog(command, message) {
  const apps = await runOperation(message, () => invoke(command));
  if (!apps) return;
  advancedAppCatalog = apps;
  byId("advanced-apps-list-status").textContent = apps.length === 0
    ? "没有找到可用的 Linux 应用标识。"
    : `已读取 ${apps.length} 个应用标识；点击即可加入规则。`;
  renderAppCatalog();
}

function renderAppCatalog() {
  const container = byId("advanced-running-apps");
  const query = byId("advanced-search-apps").value.trim().toLocaleLowerCase("zh-CN");
  const apps = advancedAppCatalog.filter((app) => {
    const haystack = `${app.process_name ?? ""} ${app.title ?? ""}`.toLocaleLowerCase("zh-CN");
    return !query || haystack.includes(query);
  });
  container.replaceChildren();
  if (apps.length === 0) {
    container.textContent = advancedAppCatalog.length
      ? "没有匹配的应用标识。"
      : "点击“已安装”或“正在运行”读取候选值。";
    container.classList.add("muted");
    return;
  }
  container.classList.remove("muted");
  apps.forEach((app) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "button secondary app-choice";
    button.title = `${app.title || "无标题"} · ${app.process_name}`;
    button.textContent = `${app.title || app.process_name} · ${app.process_name}`;
    button.addEventListener("click", () => addCompatibilityApp(app.process_name));
    container.append(button);
  });
}

function addCompatibilityApp(identifier) {
  const textarea = byId("advanced-app-processes");
  const identifiers = parseLinuxAppIdentifiers(textarea.value);
  const normalized = parseLinuxAppIdentifiers(identifier)[0];
  if (normalized && !identifiers.some((item) => item.toLowerCase() === normalized.toLowerCase())) {
    identifiers.push(normalized);
  }
  textarea.value = identifiers.join("\n");
  markAdvancedDirty();
  renderSelectedApps();
  scheduleAdvancedSave();
}

function renderSelectedApps() {
  const container = byId("advanced-selected-apps");
  const summary = byId("advanced-selected-summary");
  const identifiers = parseLinuxAppIdentifiers(byId("advanced-app-processes").value);
  summary.textContent = identifiers.length === 0
    ? "尚未选择应用"
    : `已选择 ${identifiers.length} 个应用标识`;
  container.replaceChildren();
  if (identifiers.length === 0) {
    const hint = document.createElement("span");
    hint.className = "muted";
    hint.textContent = "请从下方选择，或手动添加应用标识。";
    container.append(hint);
    return;
  }
  identifiers.forEach((identifier) => {
    const pill = document.createElement("span");
    pill.className = "app-pill";
    pill.append(document.createTextNode(identifier));
    const remove = document.createElement("button");
    remove.type = "button";
    remove.setAttribute("aria-label", `移除 ${identifier}`);
    remove.textContent = "×";
    remove.addEventListener("click", () => {
      byId("advanced-app-processes").value = identifiers
        .filter((item) => item !== identifier)
        .join("\n");
      markAdvancedDirty();
      renderSelectedApps();
      scheduleAdvancedSave();
    });
    pill.append(remove);
    container.append(pill);
  });
}

function parseLinuxAppIdentifiers(value) {
  const identifiers = value
    .split(/[\r\n,;]+/)
    .map((raw) => raw.trim().replace(/^['"]|['"]$/g, "").split(/[\\/]/).pop().trim())
    // `.exe` is a valid explicit Linux identifier (for example Wine apps).
    // Never invent the suffix here, but do not silently erase a supplied one.
    .map((identifier) => identifier.replace(/\.desktop$/i, ""))
    .filter(Boolean);
  return identifiers.filter((identifier, index) =>
    identifiers.findIndex((candidate) => candidate.toLowerCase() === identifier.toLowerCase()) === index
  );
}

function renderTouchpadStatus(status) {
  const statusElement = byId("touchpad-status");
  const deviceList = byId("touchpad-devices");
  const devices = Array.isArray(status.devices) ? status.devices : [];

  if (!status.initialized) {
    setNotice(statusElement, "info", "正在初始化 Linux 触摸板服务…", "正在检查 evdev 设备和 uinput 输出。");
  } else if (!status.touchpad_exists) {
    setNotice(
      statusElement,
      "error",
      "未检测到可读取的多点触控触摸板。",
      "请检查硬件以及 /dev/input/event* 的 udev 访问权限。"
    );
  } else if (!status.receiver_installed) {
    setNotice(
      statusElement,
      "error",
      "已找到触摸板，但输入服务未就绪。",
      "请检查 /dev/uinput 权限和最近日志。"
    );
  } else {
    setNotice(
      statusElement,
      "success",
      "Linux 触摸板服务已就绪。",
      `已连接 ${devices.length} 个多点触控设备。`
    );
  }

  byId("device-count-summary").textContent = `${devices.length} 个设备`;
  deviceList.replaceChildren();
  if (devices.length === 0) {
    deviceList.textContent = "尚未检测到设备信息。";
    deviceList.classList.add("muted");
    return;
  }
  deviceList.classList.remove("muted");
  devices.forEach((device) => {
    const row = document.createElement("div");
    row.textContent = deviceLabel(device);
    deviceList.append(row);
  });
}

function renderDeviceSettings(snapshot) {
  const container = byId("device-settings");
  const devices = Array.isArray(snapshot.touchpad.devices) ? snapshot.touchpad.devices : [];
  container.replaceChildren();
  if (devices.length === 0) {
    const notice = document.createElement("div");
    notice.className = "notice";
    notice.dataset.tone = "info";
    const title = document.createElement("strong");
    title.textContent = "尚无可配置的触摸板。";
    const description = document.createElement("span");
    description.textContent = "检测到设备后会在这里显示指针移动设置。";
    notice.append(title, description);
    container.append(notice);
    return;
  }
  devices.forEach((device) => {
    const configuration = snapshot.settings.devices[device.id] ?? {
      cursor_move: true,
      cursor_speed: 30,
      cursor_acceleration: 10,
    };
    container.append(createDeviceCard(device, configuration));
  });
}

function createDeviceCard(device, configuration) {
  const card = document.createElement("article");
  card.className = "card card-stack device-card";
  card.dataset.deviceId = device.id;

  const row = document.createElement("div");
  row.className = "card-row";
  const copy = document.createElement("div");
  copy.className = "card-copy";
  const title = document.createElement("h2");
  title.className = "device-title";
  title.textContent = `启用指针移动（${deviceLabel(device)}）`;
  const description = document.createElement("p");
  description.textContent = "应用通过 uinput 移动指针；如果另一个工具已负责指针移动，请关闭此项。";
  copy.append(title, description);

  const toggleLabel = document.createElement("label");
  toggleLabel.className = "switch";
  toggleLabel.setAttribute("aria-label", `启用 ${deviceLabel(device)} 的指针移动`);
  const toggle = document.createElement("input");
  toggle.type = "checkbox";
  toggle.checked = configuration.cursor_move;
  toggle.dataset.deviceControl = "cursor-move";
  toggleLabel.append(toggle, document.createElement("span"));
  row.append(copy, toggleLabel);

  const controls = document.createElement("div");
  controls.className = "device-controls";
  controls.append(
    createDeviceNumberSetting("指针速度", "默认值：30", "speed", configuration.cursor_speed, 0, 100000, 1, 200),
    createDeviceNumberSetting("指针加速度", "默认值：10；0 表示关闭", "acceleration", configuration.cursor_acceleration, 0, 1000, 0, 30)
  );
  card.append(row, controls);
  return card;
}

function createDeviceNumberSetting(title, description, key, value, numberMin, numberMax, rangeMin, rangeMax) {
  const setting = document.createElement("div");
  setting.className = "sub-setting";
  const label = document.createElement("label");
  const strong = document.createElement("strong");
  strong.textContent = title;
  const small = document.createElement("small");
  small.textContent = description;
  label.append(strong, small);

  const inputs = document.createElement("div");
  inputs.className = "range-number";
  const range = document.createElement("input");
  range.type = "range";
  range.min = String(rangeMin);
  range.max = String(rangeMax);
  range.step = "1";
  range.value = String(Math.min(rangeMax, Math.max(rangeMin, value)));
  range.dataset.deviceControl = key;
  range.dataset.sync = key;
  const number = document.createElement("input");
  number.type = "number";
  number.min = String(numberMin);
  number.max = String(numberMax);
  number.step = "1";
  number.value = String(value);
  number.dataset.deviceControl = key;
  number.dataset.sync = key;
  inputs.append(range, number);
  setting.append(label, inputs);
  return setting;
}

function handleTouchpadEvent(event) {
  if (!event?.kind) return;
  if (event.kind === "contacts") {
    renderContactPreview(event.payload);
    return;
  }
  if (event.kind === "error") {
    setNotice(byId("touchpad-status"), "error", event.payload);
    return;
  }
  if (event.kind === "advanced") {
    if (currentSnapshot) {
      currentSnapshot.advanced_runtime = event.payload;
      // Runtime counters/capabilities may update many times while the user is
      // editing. Preserve dirty controls; only update status-only UI until the
      // queued save has committed the current revision.
      if (advancedDirty || advancedSaveInFlight) renderAdvancedStatus(currentSnapshot);
      else renderAdvanced(currentSnapshot);
    } else {
      pendingAdvancedRuntime = event.payload;
    }
    return;
  }
  if (event.kind === "status" && currentSnapshot) {
    currentSnapshot.touchpad = event.payload;
    renderTouchpadStatus(event.payload);
    clearTimeout(statusRefreshTimer);
    statusRefreshTimer = setTimeout(() => refreshSnapshot().catch(showError), 100);
  }
}

function renderContactPreview(preview) {
  const container = byId("contact-preview");
  const contacts = Array.isArray(preview.contacts) ? preview.contacts : [];
  container.replaceChildren();
  const meta = document.createElement("div");
  meta.className = "contact-meta";
  meta.textContent = `${deviceLabel(preview.device)} · ${contacts.length} 个接触点 · 间隔 ${preview.event_interval_ms} ms`;
  const points = document.createElement("div");
  points.className = "contact-points";
  if (contacts.length === 0) {
    points.textContent = "当前没有手指接触。";
  } else {
    contacts.forEach((contact) => {
      const point = document.createElement("span");
      point.className = "contact-point";
      point.textContent = `ID ${contact.id}: X ${contact.x}, Y ${contact.y}`;
      points.append(point);
    });
  }
  container.append(meta, points);
}

function updateGestureControlState() {
  const enabled = byId("three-finger-enabled").checked;
  const releaseEnabled = byId("allow-release").checked;
  byId("drag-button").disabled = !enabled;
  byId("allow-release").disabled = !enabled;
  byId("release-delay").disabled = !enabled || !releaseEnabled;
  document.querySelectorAll("[data-pair], #cursor-averaging").forEach((control) => {
    control.disabled = !enabled;
  });
  document.querySelectorAll(".device-card").forEach((card) => {
    const movementToggle = card.querySelector('[data-device-control="cursor-move"]');
    movementToggle.disabled = !enabled;
    card.querySelectorAll('[data-device-control="speed"], [data-device-control="acceleration"]').forEach((control) => {
      control.disabled = !enabled || !movementToggle.checked;
    });
  });
}

function synchronizePair(source) {
  document.querySelectorAll(`[data-pair="${source.dataset.pair}"]`).forEach((control) => {
    if (control !== source) control.value = source.value;
  });
}

function synchronizeDevicePair(source) {
  const card = source.closest(".device-card");
  card.querySelectorAll(`[data-sync="${source.dataset.sync}"]`).forEach((control) => {
    if (control === source) return;
    const numeric = finiteNumber(source.value, 0);
    control.value = String(Math.min(finiteNumber(control.max, numeric), Math.max(finiteNumber(control.min, numeric), numeric)));
  });
}

function enforceThresholdRelationship(changedPair) {
  const start = finiteNumber(byId("start-threshold").value, 0);
  const stop = finiteNumber(byId("stop-threshold").value, 0);
  if (changedPair === "start-threshold" && start < stop) setPairValue("stop-threshold", start);
  if (changedPair === "stop-threshold" && stop > start) setPairValue("start-threshold", stop);
}

function setPairValue(pair, value) {
  document.querySelectorAll(`[data-pair="${pair}"]`).forEach((control) => {
    control.value = String(Math.min(finiteNumber(control.max, value), Math.max(finiteNumber(control.min, value), value)));
  });
}

function scheduleGestureSave() {
  if (rendering || !currentSnapshot) return;
  clearTimeout(saveTimer);
  showStatus("等待保存基础拖动设置…");
  saveTimer = setTimeout(() => saveGestureSettings().catch(showError), 220);
}

function scheduleAdvancedSave() {
  if (rendering || !currentSnapshot) return;
  clearTimeout(advancedSaveTimer);
  showStatus("等待保存高级窗口手势设置…");
  advancedSaveTimer = setTimeout(() => requestAdvancedSave(), 220);
}

function requestAdvancedSave() {
  if (advancedSaveInFlight) {
    advancedSaveQueued = true;
    return;
  }
  saveAdvancedSettings().catch(showError);
}

async function saveAdvancedSettings() {
  if (!advancedDirty || !currentSnapshot) return;
  const input = collectAdvancedSettings();
  const revision = advancedEditRevision;
  advancedSaveInFlight = true;
  showStatus(input.enabled
    ? "正在请求 broker 启用并保存高级设置…"
    : "正在保存高级窗口手势设置…");
  try {
    const snapshot = await invoke("save_advanced_settings", { input });
    // An older response must not overwrite edits made while it was in flight.
    // It may still refresh status-only data; the queued save owns the controls.
    if (revision === advancedEditRevision) {
      currentSnapshot = snapshot;
      advancedDirty = false;
      renderAdvanced(currentSnapshot);
      showStatus(input.enabled
        ? "高级设置已保存，等待运行态确认。"
        : "高级窗口手势设置已保存。");
    } else {
      currentSnapshot.advanced_runtime = snapshot.advanced_runtime;
      currentSnapshot.integration = snapshot.integration;
      renderAdvancedStatus(currentSnapshot);
      advancedSaveQueued = true;
    }
  } catch (error) {
    // Backend rejection is authoritative.  Re-render the last confirmed
    // snapshot so a failed enable request never remains visually checked.
    if (revision === advancedEditRevision) {
      advancedDirty = false;
      renderAdvanced(currentSnapshot);
    }
    throw error;
  } finally {
    advancedSaveInFlight = false;
    if (advancedSaveQueued || advancedDirty) {
      advancedSaveQueued = false;
      queueMicrotask(requestAdvancedSave);
    }
  }
}

function collectAdvancedSettings() {
  const original = currentSnapshot.advanced ?? {};
  const input = { ...original };
  document.querySelectorAll("[data-advanced-control]").forEach((control) => {
    const key = control.dataset.advancedControl;
    if (control.type === "checkbox") input[key] = control.checked;
    else if (["number", "range"].includes(control.type)) {
      input[key] = finiteNumber(control.value, original[key]);
    } else {
      input[key] = control.value;
    }
  });

  // Linux exposes one explicit runtime switch.  The internal gesture-engine
  // switch follows it, while persistent startup switches remain unavailable.
  // The source model intentionally keeps the engine master independent from
  // module installation/availability. Preserve that preference on Linux even
  // though only the outer `enabled` switch is exposed in this compact UI.
  input.gesturesEnabled = original.gesturesEnabled !== false;
  input.onboardingCompleted = Boolean(original.onboardingCompleted);
  input.enableOnAppStart = false;
  input.launchAtLogin = false;
  input.fiveFingerEnabled = false;
  input.centerEnabled = false;
  // Removed optional interactions stay fail-closed even when an older saved
  // profile had enabled them. Directional snap and its final commit are the
  // only two-finger window behavior exposed by this Linux UI.
  input.livePreview = false;
  input.moveCursor = false;
  input.mouseMiddleButtonHudEnabled = false;
  input.resizeHorizontalEnabled = false;
  input.resizeVerticalEnabled = false;
  const monitor = byId("advanced-monitor-modifier").value;
  input.monitorMoveEnabled = monitor !== "off";
  if (monitor !== "off") input.monitorMoveModifier = monitor;
  input.appCompatibilityProcessNames = parseLinuxAppIdentifiers(
    byId("advanced-app-processes").value
  );
  delete input.appCompatibilityProcessNamesText;
  return input;
}

async function saveGestureSettings() {
  showStatus("正在保存基础拖动设置…");
  currentSnapshot = await invoke("save_gesture_settings", { input: collectGestureSettings() });
  renderSnapshot(currentSnapshot);
  showStatus("基础拖动设置已保存。");
}

function collectGestureSettings() {
  const devices = JSON.parse(JSON.stringify(currentSnapshot.settings.devices ?? {}));
  document.querySelectorAll(".device-card").forEach((card) => {
    const id = card.dataset.deviceId;
    devices[id] = {
      cursor_move: card.querySelector('[data-device-control="cursor-move"]').checked,
      cursor_speed: finiteNumber(card.querySelector('input[type="number"][data-device-control="speed"]').value, 30),
      cursor_acceleration: finiteNumber(card.querySelector('input[type="number"][data-device-control="acceleration"]').value, 10),
    };
  });
  return {
    three_finger_drag: byId("three-finger-enabled").checked,
    drag_button: byId("drag-button").value,
    allow_release_and_restart: byId("allow-release").checked,
    release_delay_ms: finiteNumber(byId("release-delay").value, 500),
    devices,
    cursor_averaging: finiteNumber(byId("cursor-averaging").value, 1),
    max_finger_move_distance: finiteNumber(byId("max-move-distance").value, 0),
    start_threshold: finiteNumber(byId("start-threshold").value, 100),
    stop_threshold: finiteNumber(byId("stop-threshold").value, 10),
  };
}

async function runSnapshotToggle(control, command, message) {
  const requested = control.checked;
  control.disabled = true;
  try {
    showStatus(message);
    currentSnapshot = await invoke(command, { enabled: requested });
    renderSnapshot(currentSnapshot);
    showStatus("设置已更新。");
  } catch (error) {
    control.checked = !requested;
    showError(error);
  } finally {
    control.disabled = false;
  }
}

async function runOperation(message, operation) {
  try {
    showStatus(message);
    const result = await operation();
    showStatus("操作已完成。");
    return result;
  } catch (error) {
    showError(error);
    return null;
  }
}

async function copyText(value) {
  try {
    await navigator.clipboard.writeText(value);
  } catch {
    const fallback = document.createElement("textarea");
    fallback.value = value;
    fallback.setAttribute("readonly", "");
    fallback.className = "clipboard-fallback";
    document.body.append(fallback);
    fallback.select();
    document.execCommand("copy");
    fallback.remove();
  }
}

function setNotice(element, tone, title, description = "") {
  element.dataset.tone = tone;
  element.replaceChildren();
  const strong = document.createElement("strong");
  strong.textContent = title;
  element.append(strong);
  if (description) {
    const span = document.createElement("span");
    span.textContent = description;
    element.append(span);
  }
}

function showStatus(message) {
  const status = byId("operation-status");
  status.textContent = message;
  status.dataset.error = "false";
}

function showError(error) {
  const message = typeof error === "string" ? error : error?.message ?? String(error);
  const status = byId("operation-status");
  status.textContent = `操作失败：${message}`;
  status.dataset.error = "true";
}

function finiteNumber(value, fallback) {
  const number = Number(value);
  return Number.isFinite(number) ? number : fallback;
}

function deviceLabel(device) {
  return `${device.id}（产品 ${device.product_id}，厂商 ${device.vendor_id}）`;
}
