"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

let currentSnapshot = null;
let rendering = false;
let saveTimer = null;
let advancedSaveTimer = null;
let statusRefreshTimer = null;
let advancedAppCatalog = [];
let advancedTutorialIndex = 0;

const advancedTutorialSteps = [
  {
    title: "欢迎使用 Swoosh",
    description: "将光标移到窗口标题栏，然后在触摸板上用双指向右滑动，即可把窗口吸附到右半屏。抬起手指完成，按 Esc 可取消。",
    swipe: "right",
    demo: "snap",
  },
  {
    title: "最大化与最小化",
    description: "双指向上滑动可最大化窗口，向下滑动可最小化窗口。",
    swipe: "up",
    demo: "snap",
  },
  {
    title: "吸附到角落",
    description: "沿对角线滑动，可把窗口吸附到屏幕对应的四分之一区域。",
    swipe: "up-left",
    demo: "snap",
  },
  {
    title: "最小化或关闭",
    description: "把“向下滑动”设为“由我选择”后，向下滑动会展开选择器；向左选择最小化，向右选择红色关闭按钮。",
    swipe: "down",
    demo: "chooser",
  },
  {
    title: "用五指调整窗口",
    description: "五指放在触摸板上，张开可放大窗口，捏合可缩小窗口；轻点可让窗口居中。",
    swipe: "none",
    demo: "resize",
  },
  {
    title: "切换虚拟桌面或显示器",
    description: "双指静止片刻后再滑动，可把窗口移到其他虚拟桌面或物理显示器；HUD 会预览最终落点。",
    swipe: "right",
    demo: "desktop",
  },
  {
    title: "一切就绪",
    description: "按住 Shift 再滑动可进行三等分吸附。你随时都能回到主页重新播放本教程。",
    swipe: "none",
    demo: "done",
  },
];

const byId = (id) => document.getElementById(id);

document.addEventListener("DOMContentLoaded", () => {
  bindTabs();
  bindGestureControls();
  bindAdvancedControls();
  bindApplicationControls();
  initialize().catch(showError);
});

async function initialize() {
  await refreshSnapshot();
  await listen("touchpad-event", ({ payload }) => {
    handleTouchpadEvent(payload);
  });
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
  const directControls = [
    "three-finger-enabled",
    "drag-button",
    "allow-release",
    "release-delay",
    "cursor-averaging",
  ];
  directControls.forEach((id) => {
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
    if (!control) {
      return;
    }
    if (control.dataset.sync) {
      synchronizeDevicePair(control);
    }
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

function bindApplicationControls() {
  byId("open-touchpad-settings").addEventListener("click", async () => {
    await runOperation("正在打开 Windows 触摸板设置…", () =>
      invoke("open_touchpad_settings")
    );
  });

  byId("run-at-startup").addEventListener("change", async (event) => {
    await runSnapshotToggle(
      event.target,
      "set_run_at_startup",
      "正在更新开机启动设置…"
    );
  });

  byId("run-elevated").addEventListener("change", async (event) => {
    await runSnapshotToggle(
      event.target,
      "set_run_elevated",
      "正在更新管理员运行设置…"
    );
  });

  byId("record-logs").addEventListener("change", async (event) => {
    await runSnapshotToggle(
      event.target,
      "set_record_logs",
      "正在更新日志设置…"
    );
  });

  byId("save-logs").addEventListener("click", async () => {
    const path = await runOperation("正在保存日志…", () => invoke("save_logs"));
    if (path) {
      showStatus(`日志已保存到：${path}`);
    }
  });

  document.querySelectorAll("[data-url]").forEach((button) => {
    button.addEventListener("click", async () => {
      await runOperation("正在打开链接…", () =>
        invoke("open_external", { url: button.dataset.url })
      );
    });
  });

  byId("close-settings").addEventListener("click", async () => {
    await invoke("close_settings");
  });

  byId("restore-advanced-defaults").addEventListener("click", async () => {
    const confirmed = window.confirm("只恢复进阶手势设置，确定继续吗？");
    if (!confirmed) return;
    await runOperation("正在恢复进阶默认设置…", async () => {
      currentSnapshot = await invoke("restore_advanced_defaults");
      renderSnapshot(currentSnapshot);
      return currentSnapshot;
    });
  });

  const restoreAbout = byId("restore-advanced-defaults-about");
  if (restoreAbout) {
    restoreAbout.addEventListener("click", async () => {
      const confirmed = window.confirm("只恢复进阶手势设置，确定继续吗？");
      if (!confirmed) return;
      await runOperation("正在恢复进阶默认设置…", async () => {
        currentSnapshot = await invoke("restore_advanced_defaults");
        renderSnapshot(currentSnapshot);
        return currentSnapshot;
      });
    });
  }

  const runningAppsButton = byId("advanced-list-running-apps");
  if (runningAppsButton) runningAppsButton.addEventListener("click", () => loadAppCatalog("list_running_apps", "正在读取运行中的应用…"));
  const installedAppsButton = byId("advanced-list-installed-apps");
  if (installedAppsButton) installedAppsButton.addEventListener("click", () => loadAppCatalog("list_installed_apps", "正在读取已安装应用…"));
  const appSearch = byId("advanced-search-apps");
  if (appSearch) appSearch.addEventListener("input", renderAppCatalog);

  const diagnosticsButton = byId("copy-advanced-diagnostics");
  if (diagnosticsButton) {
    diagnosticsButton.addEventListener("click", async () => {
      const report = await runOperation("正在生成诊断…", () => invoke("advanced_diagnostics"));
      if (!report) return;
      try {
        await navigator.clipboard.writeText(report);
        showStatus("诊断信息已复制到剪贴板。");
      } catch {
        const fallback = document.createElement("textarea");
        fallback.value = report;
        document.body.append(fallback);
        fallback.select();
        document.execCommand("copy");
        fallback.remove();
        showStatus("诊断信息已复制到剪贴板。");
      }
    });
  }

  const tutorialButton = byId("replay-advanced-tutorial");
  if (tutorialButton) {
    tutorialButton.addEventListener("click", openAdvancedTutorial);
  }

  const tutorial = byId("advanced-tutorial");
  byId("advanced-tutorial-back")?.addEventListener("click", () => {
    advancedTutorialIndex = Math.max(0, advancedTutorialIndex - 1);
    renderAdvancedTutorialStep();
  });
  byId("advanced-tutorial-next")?.addEventListener("click", () => {
    if (advancedTutorialIndex >= advancedTutorialSteps.length - 1) {
      tutorial?.close();
      return;
    }
    advancedTutorialIndex += 1;
    renderAdvancedTutorialStep();
  });
  byId("advanced-tutorial-skip")?.addEventListener("click", () => tutorial?.close());
  tutorial?.addEventListener("cancel", () => {
    advancedTutorialIndex = 0;
  });

  const reportButton = byId("report-advanced-problem");
  if (reportButton) {
    reportButton.addEventListener("click", async () => {
      const diagnostics = await runOperation("正在生成诊断并打开报告页面…", () =>
        invoke("advanced_diagnostics")
      );
      if (!diagnostics) return;
      const body = `## 发生了什么？\n\n_请描述问题和复现步骤。_\n\n## 诊断信息\n\`\`\`\n${diagnostics}\n\`\`\`\n`;
      const url = `https://github.com/bwya77/swoosh/issues/new?labels=beta&title=${encodeURIComponent("[Beta] ")}&body=${encodeURIComponent(body)}`;
      await runOperation("正在打开 GitHub 问题页面…", () =>
        invoke("open_external", { url })
      );
    });
  }
}

function openAdvancedTutorial() {
  const tutorial = byId("advanced-tutorial");
  if (!tutorial) return;
  advancedTutorialIndex = 0;
  renderAdvancedTutorialStep();
  tutorial.showModal();
}

function renderAdvancedTutorialStep() {
  const step = advancedTutorialSteps[advancedTutorialIndex];
  if (!step) return;
  byId("advanced-tutorial-title").textContent = step.title;
  byId("advanced-tutorial-description").textContent = step.description;
  const demo = byId("advanced-tutorial-demo");
  demo.dataset.swipe = step.swipe;
  demo.dataset.demo = step.demo;
  byId("advanced-tutorial-back").disabled = advancedTutorialIndex === 0;
  byId("advanced-tutorial-next").textContent =
    advancedTutorialIndex === advancedTutorialSteps.length - 1 ? "完成" : "下一步";
  const dots = byId("advanced-tutorial-dots");
  dots.replaceChildren();
  advancedTutorialSteps.forEach((_, index) => {
    const dot = document.createElement("i");
    dot.classList.toggle("is-active", index === advancedTutorialIndex);
    dots.append(dot);
  });
}

function bindAdvancedControls() {
  document.querySelectorAll("[data-advanced-control]").forEach((control) => {
    control.addEventListener("change", () => {
      updateAdvancedDerivedUi();
      scheduleAdvancedSave();
    });
    if (["number", "range", "color"].includes(control.type) || control.tagName === "TEXTAREA") {
      control.addEventListener("input", () => {
        updateAdvancedDerivedUi();
        if (control.id === "advanced-app-processes") renderSelectedApps();
        scheduleAdvancedSave();
      });
    }
  });
  const monitorControl = byId("advanced-monitor-modifier");
  if (monitorControl) monitorControl.addEventListener("change", scheduleAdvancedSave);
  document.querySelectorAll("[data-overlay-color]").forEach((button) => {
    button.addEventListener("click", () => {
      byId("advanced-overlay-color").value = button.dataset.overlayColor;
      byId("advanced-overlay-accent").checked = false;
      updateAdvancedDerivedUi();
      scheduleAdvancedSave();
    });
  });
}

async function refreshSnapshot() {
  currentSnapshot = await invoke("get_snapshot");
  renderSnapshot(currentSnapshot);
}

function renderSnapshot(snapshot) {
  rendering = true;
  const settings = snapshot.settings;

  byId("three-finger-enabled").checked = settings.three_finger_drag;
  byId("drag-button").value = settings.drag_button;
  byId("allow-release").checked = settings.allow_release_and_restart;
  byId("release-delay").value = settings.release_delay_ms;
  setPairValue("start-threshold", settings.start_threshold);
  setPairValue("stop-threshold", settings.stop_threshold);
  setPairValue("max-move-distance", settings.max_finger_move_distance);
  byId("cursor-averaging").value = settings.cursor_averaging;

  byId("run-at-startup").checked = snapshot.startup.enabled;
  byId("run-elevated").checked = settings.run_elevated;
  byId("record-logs").checked = settings.record_logs;
  byId("app-version").textContent = `版本 ${snapshot.version}`;

  renderAdvanced(snapshot);

  renderTouchpadStatus(snapshot.touchpad);
  renderDeviceSettings(snapshot);
  renderStartupStatus(snapshot);
  updateGestureControlState();
  rendering = false;
}

function renderAdvanced(snapshot) {
  const settings = snapshot.advanced ?? {};
  const values = {
    enabled: settings.enabled,
    gesturesEnabled: settings.gesturesEnabled,
    enableOnAppStart: settings.enableOnAppStart,
    launchAtLogin: settings.launchAtLogin,
    phantomRejection: settings.phantomRejection,
    livePreview: settings.livePreview,
    animateSnaps: settings.animateSnaps,
    snapAnimationSeconds: settings.snapAnimationSeconds,
    halvesEnabled: settings.halvesEnabled,
    maximizeEnabled: settings.maximizeEnabled,
    quartersEnabled: settings.quartersEnabled,
    minimizeEnabled: settings.minimizeEnabled,
    swipeDownAction: settings.swipeDownAction,
    swipeDownThreshold: settings.swipeDownThreshold,
    centerEnabled: settings.centerEnabled,
    gridModifierEnabled: settings.gridModifierEnabled,
    gridModifier: settings.gridModifier,
    sensitivity: settings.sensitivity,
    gridSpacing: settings.gridSpacing,
    taskbarIconGesturesEnabled: settings.taskbarIconGesturesEnabled,
    fiveFingerEnabled: settings.fiveFingerEnabled,
    resizeHorizontalEnabled: settings.resizeHorizontalEnabled,
    resizeVerticalEnabled: settings.resizeVerticalEnabled,
    moveCursor: settings.moveCursor,
    mouseMiddleButtonHudEnabled: settings.mouseMiddleButtonHudEnabled,
    monitorMoveEnabled: settings.monitorMoveEnabled,
    monitorMoveModifier: settings.monitorMoveModifier,
    appSwitchOnHold: settings.appSwitchOnHold,
    previewDesktopDestination: settings.previewDesktopDestination,
    createDesktopOnOverflow: settings.createDesktopOnOverflow,
    desktopHoldDelaySeconds: settings.desktopHoldDelaySeconds,
    cancelTimeoutSeconds: settings.cancelTimeoutSeconds,
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
    if (!(key in values)) return;
    if (control.type === "checkbox") control.checked = Boolean(values[key]);
    else control.value = String(values[key]);
  });
  const monitorControl = byId("advanced-monitor-modifier");
  if (monitorControl) {
    monitorControl.value = settings.monitorMoveEnabled === false
      ? "off"
      : (settings.monitorMoveModifier ?? "alt");
  }
  updateAdvancedDerivedUi();
  renderSelectedApps();

  const runtime = snapshot.advanced_runtime ?? {};
  const lifetime = byId("advanced-lifetime-swooshes");
  if (lifetime) lifetime.textContent = Number(runtime.completedSwooshes ?? 0).toLocaleString("zh-CN");
  const status = byId("advanced-status");
  if (runtime.lastError) {
    setNotice(status, "error", "进阶 Rust 引擎已自动停用。", runtime.lastError);
  } else if (settings.enabled && settings.gesturesEnabled !== false && runtime.available !== false) {
    setNotice(status, "success", "进阶 Rust 引擎已启用。", `已处理 ${runtime.processed_frames ?? 0} 个输入帧。`);
  } else {
    setNotice(status, "info", "进阶手势当前关闭。", "基础三指拖动仍独立运行。 ");
  }
}

function updateAdvancedDerivedUi() {
  document.querySelectorAll("[data-output-for]").forEach((output) => {
    const control = byId(output.dataset.outputFor);
    if (!control) return;
    const suffix = control.id === "advanced-grid-spacing" ? " px" : " 秒";
    if (control.id === "advanced-sensitivity" || control.id === "advanced-swipe-down-threshold") {
      output.textContent = Number(control.value).toFixed(2);
    } else if (control.id === "advanced-grid-spacing") {
      output.textContent = `${Math.round(Number(control.value))}${suffix}`;
    } else {
      output.textContent = `${Number(control.value).toFixed(2)}${suffix}`;
    }
  });
  const modifierRow = byId("advanced-app-modifier-row");
  if (modifierRow) modifierRow.hidden = byId("advanced-app-compat-mode")?.value !== "requireModifier";
  const color = byId("advanced-overlay-color")?.value?.toUpperCase();
  document.querySelectorAll("[data-overlay-color]").forEach((button) => {
    button.classList.toggle("is-selected", button.dataset.overlayColor === color);
  });
}

async function loadAppCatalog(command, message) {
  const apps = await runOperation(message, () => invoke(command));
  if (!apps) return;
  advancedAppCatalog = apps;
  byId("advanced-apps-list-status").textContent = apps.length === 0
    ? "没有找到可用应用。便携应用可先运行，再点击“正在运行”。"
    : `已读取 ${apps.length} 个应用；点击应用即可加入兼容列表。`;
  renderAppCatalog();
}

function renderAppCatalog() {
  const container = byId("advanced-running-apps");
  if (!container) return;
  const query = (byId("advanced-search-apps")?.value ?? "").trim().toLocaleLowerCase("zh-CN");
  const apps = advancedAppCatalog.filter((app) => {
    const haystack = `${app.process_name ?? ""} ${app.title ?? ""}`.toLocaleLowerCase("zh-CN");
    return !query || haystack.includes(query);
  });
  container.replaceChildren();
  if (apps.length === 0) {
    container.textContent = advancedAppCatalog.length ? "没有匹配的应用。" : "点击“已安装”或“正在运行”来加载应用。";
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

function addCompatibilityApp(processName) {
  const textarea = byId("advanced-app-processes");
  if (!textarea) return;
  const names = parseProcessList(textarea.value);
  const normalized = parseProcessList(processName)[0];
  if (normalized && !names.includes(normalized)) names.push(normalized);
  textarea.value = names.join("\n");
  renderSelectedApps();
  scheduleAdvancedSave();
}

function renderSelectedApps() {
  const container = byId("advanced-selected-apps");
  const summary = byId("advanced-selected-summary");
  if (!container || !summary) return;
  const names = parseProcessList(byId("advanced-app-processes")?.value ?? "");
  summary.textContent = names.length === 0 ? "尚未选择应用" : `已选择 ${names.length} 个应用`;
  container.replaceChildren();
  if (names.length === 0) {
    const hint = document.createElement("span");
    hint.className = "muted";
    hint.textContent = "请从下方选择应用，或在“其他应用”中添加。";
    container.append(hint);
    return;
  }
  names.forEach((name) => {
    const pill = document.createElement("span");
    pill.className = "app-pill";
    pill.append(document.createTextNode(name));
    const remove = document.createElement("button");
    remove.type = "button";
    remove.setAttribute("aria-label", `移除 ${name}`);
    remove.textContent = "×";
    remove.addEventListener("click", () => {
      byId("advanced-app-processes").value = names.filter((item) => item !== name).join("\n");
      renderSelectedApps();
      scheduleAdvancedSave();
    });
    pill.append(remove);
    container.append(pill);
  });
}

function renderTouchpadStatus(status) {
  const statusElement = byId("touchpad-status");
  const deviceList = byId("touchpad-devices");

  if (!status.initialized) {
    setNotice(statusElement, "info", "正在初始化触摸板服务…", "请稍候。");
  } else if (!status.touchpad_exists) {
    setNotice(
      statusElement,
      "error",
      "未检测到触摸板。",
      "请确认设备配有 Windows 精确式触摸板。"
    );
  } else if (!status.receiver_installed) {
    setNotice(
      statusElement,
      "warning",
      "已检测到触摸板，但无法注册输入接收器。",
      "请重新启动应用后再试。"
    );
  } else {
    setNotice(
      statusElement,
      "success",
      "已检测并成功连接触摸板。",
      `已连接 ${status.devices.length} 个设备。`
    );
  }

  deviceList.replaceChildren();
  if (status.devices.length === 0) {
    deviceList.textContent = "尚未检测到设备信息。";
    deviceList.classList.add("muted");
    return;
  }

  deviceList.classList.remove("muted");
  status.devices.forEach((device) => {
    const row = document.createElement("div");
    row.textContent = deviceLabel(device);
    deviceList.append(row);
  });
}

function renderDeviceSettings(snapshot) {
  const container = byId("device-settings");
  container.replaceChildren();

  if (snapshot.touchpad.devices.length === 0) {
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

  snapshot.touchpad.devices.forEach((device) => {
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
  title.textContent = `启用三指指针移动（${deviceLabel(device)}）`;
  const description = document.createElement("p");
  description.textContent =
    "应用自行移动指针，手感可能与单指操作略有不同；已有其他方案时可关闭。";
  copy.append(title, description);

  const toggleLabel = document.createElement("label");
  toggleLabel.className = "switch";
  toggleLabel.setAttribute("aria-label", `启用 ${deviceLabel(device)} 的指针移动`);
  const toggle = document.createElement("input");
  toggle.type = "checkbox";
  toggle.checked = configuration.cursor_move;
  toggle.dataset.deviceControl = "cursor-move";
  const toggleVisual = document.createElement("span");
  toggleLabel.append(toggle, toggleVisual);
  row.append(copy, toggleLabel);

  const controls = document.createElement("div");
  controls.className = "device-controls";
  controls.append(
    createDeviceNumberSetting(
      "指针速度",
      "默认值：30",
      "speed",
      configuration.cursor_speed,
      0,
      100000,
      1,
      200
    ),
    createDeviceNumberSetting(
      "指针加速度",
      "默认值：10；设为 0 可关闭",
      "acceleration",
      configuration.cursor_acceleration,
      0,
      1000,
      0,
      30
    )
  );

  card.append(row, controls);
  return card;
}

function createDeviceNumberSetting(
  title,
  description,
  key,
  value,
  numberMin,
  numberMax,
  rangeMin,
  rangeMax
) {
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

function renderStartupStatus(snapshot) {
  setNotice(
    byId("startup-status"),
    snapshot.startup.severity,
    snapshot.startup.title
  );

  if (snapshot.is_administrator) {
    setNotice(
      byId("elevated-status"),
      "success",
      "应用当前以管理员身份运行。"
    );
  } else if (snapshot.settings.run_elevated) {
    setNotice(
      byId("elevated-status"),
      "warning",
      "管理员权限启动未完成。",
      "应用仍在普通权限下运行；再次启动时会请求 UAC 权限。"
    );
  } else {
    setNotice(
      byId("elevated-status"),
      "info",
      "应用当前以普通权限运行。"
    );
  }
}

function handleTouchpadEvent(event) {
  if (!event || !event.kind) {
    return;
  }
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
      renderAdvanced(currentSnapshot);
    }
    return;
  }
  if (event.kind === "status") {
    if (currentSnapshot) {
      currentSnapshot.touchpad = event.payload;
      renderTouchpadStatus(event.payload);
    }
    clearTimeout(statusRefreshTimer);
    statusRefreshTimer = setTimeout(() => {
      refreshSnapshot().catch(showError);
    }, 100);
  }
}

function renderContactPreview(preview) {
  const container = byId("contact-preview");
  container.replaceChildren();

  const meta = document.createElement("div");
  meta.className = "contact-meta";
  meta.textContent =
    `${deviceLabel(preview.device)} · ${preview.contacts.length} 个接触点` +
    ` · 平均间隔 ${preview.event_interval_ms} ms`;
  const points = document.createElement("div");
  points.className = "contact-points";
  if (preview.contacts.length === 0) {
    points.textContent = "当前没有手指接触。";
  } else {
    preview.contacts.forEach((contact) => {
      const point = document.createElement("span");
      point.className = "contact-point";
      point.textContent = `ID ${contact.id}: X ${contact.x}, Y ${contact.y}`;
      points.append(point);
    });
  }
  container.append(meta, points);
}

function updateGestureControlState() {
  const gestureEnabled = byId("three-finger-enabled").checked;
  const releaseEnabled = byId("allow-release").checked;
  byId("allow-release").disabled = !gestureEnabled;
  byId("release-delay").disabled = !gestureEnabled || !releaseEnabled;

  document.querySelectorAll(".device-card").forEach((card) => {
    const movementToggle = card.querySelector(
      '[data-device-control="cursor-move"]'
    );
    movementToggle.disabled = !gestureEnabled;
    const movementEnabled = movementToggle.checked;
    card.querySelectorAll(
      '[data-device-control="speed"], [data-device-control="acceleration"]'
    ).forEach((control) => {
      control.disabled = !gestureEnabled || !movementEnabled;
    });
  });
}

function synchronizePair(source) {
  document.querySelectorAll(`[data-pair="${source.dataset.pair}"]`).forEach((control) => {
    if (control !== source) {
      control.value = source.value;
    }
  });
}

function synchronizeDevicePair(source) {
  const card = source.closest(".device-card");
  card.querySelectorAll(`[data-sync="${source.dataset.sync}"]`).forEach((control) => {
    if (control !== source) {
      const numeric = finiteNumber(source.value, 0);
      const minimum = finiteNumber(control.min, numeric);
      const maximum = finiteNumber(control.max, numeric);
      control.value = String(Math.min(maximum, Math.max(minimum, numeric)));
    }
  });
}

function enforceThresholdRelationship(changedPair) {
  if (changedPair !== "start-threshold" && changedPair !== "stop-threshold") {
    return;
  }
  const start = finiteNumber(byId("start-threshold").value, 0);
  const stop = finiteNumber(byId("stop-threshold").value, 0);
  if (changedPair === "start-threshold" && start < stop) {
    setPairValue("stop-threshold", start);
  } else if (changedPair === "stop-threshold" && stop > start) {
    setPairValue("start-threshold", stop);
  }
}

function setPairValue(pair, value) {
  document.querySelectorAll(`[data-pair="${pair}"]`).forEach((control) => {
    const minimum = finiteNumber(control.min, value);
    const maximum = finiteNumber(control.max, value);
    control.value = String(Math.min(maximum, Math.max(minimum, value)));
  });
}

function scheduleGestureSave() {
  if (rendering || !currentSnapshot) {
    return;
  }
  clearTimeout(saveTimer);
  showStatus("正在等待保存…");
  saveTimer = setTimeout(() => {
    saveGestureSettings().catch(showError);
  }, 220);
}

function scheduleAdvancedSave() {
  if (rendering || !currentSnapshot) return;
  clearTimeout(advancedSaveTimer);
  showStatus("正在等待保存进阶设置…");
  advancedSaveTimer = setTimeout(() => {
    saveAdvancedSettings().catch(showError);
  }, 220);
}

async function saveAdvancedSettings() {
  const input = collectAdvancedSettings();
  showStatus("正在保存进阶设置…");
  currentSnapshot = await invoke("save_advanced_settings", { input });
  renderAdvanced(currentSnapshot);
  showStatus("进阶设置已保存。 ");
}

function collectAdvancedSettings() {
  const original = currentSnapshot.advanced ?? {};
  const input = { ...original };
  document.querySelectorAll("[data-advanced-control]").forEach((control) => {
    const key = control.dataset.advancedControl;
    if (control.type === "checkbox") input[key] = control.checked;
    else if (["number", "range"].includes(control.type)) input[key] = finiteNumber(control.value, original[key]);
    else input[key] = control.value;
  });
  // The reference UI exposes a single master switch and one combined
  // Off/Shift/Ctrl/Alt monitor selector.
  input.gesturesEnabled = Boolean(input.enabled);
  const monitor = byId("advanced-monitor-modifier")?.value ?? "off";
  input.monitorMoveEnabled = monitor !== "off";
  if (monitor !== "off") input.monitorMoveModifier = monitor;
  input.appCompatibilityProcessNames = parseProcessList(byId("advanced-app-processes")?.value ?? "");
  delete input.appCompatibilityProcessNamesText;
  return input;
}

function parseProcessList(value) {
  const names = value
    .split(/[\r\n,;]+/)
    .map((raw) => raw.trim().replace(/^"|"$/g, "").split(/[\\/]/).pop().trim())
    .filter(Boolean)
    .map((name) => name.includes(".") ? name.toLowerCase() : `${name}.exe`.toLowerCase());
  return [...new Set(names)];
}

async function saveGestureSettings() {
  const input = collectGestureSettings();
  showStatus("正在保存设置…");
  currentSnapshot = await invoke("save_gesture_settings", { input });
  showStatus("设置已保存。");
}

function collectGestureSettings() {
  const devices = structuredClone(currentSnapshot.settings.devices);
  document.querySelectorAll(".device-card").forEach((card) => {
    const id = card.dataset.deviceId;
    devices[id] = {
      cursor_move: card.querySelector(
        '[data-device-control="cursor-move"]'
      ).checked,
      cursor_speed: finiteNumber(
        card.querySelector(
          'input[type="number"][data-device-control="speed"]'
        ).value,
        30
      ),
      cursor_acceleration: finiteNumber(
        card.querySelector(
          'input[type="number"][data-device-control="acceleration"]'
        ).value,
        10
      ),
    };
  });

  return {
    three_finger_drag: byId("three-finger-enabled").checked,
    drag_button: byId("drag-button").value,
    allow_release_and_restart: byId("allow-release").checked,
    release_delay_ms: finiteNumber(byId("release-delay").value, 500),
    devices,
    cursor_averaging: finiteNumber(byId("cursor-averaging").value, 1),
    max_finger_move_distance: finiteNumber(
      byId("max-move-distance").value,
      0
    ),
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
  const message =
    typeof error === "string" ? error : error?.message ?? String(error);
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
