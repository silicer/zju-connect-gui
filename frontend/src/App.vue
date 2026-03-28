<script lang="ts" setup>
import {computed, nextTick, onMounted, onUnmounted, ref, watch} from 'vue';
import {EventsOn, WindowIsMaximised, WindowMinimise, WindowToggleMaximise} from '../wailsjs/runtime/runtime';
import type {main} from '../wailsjs/go/models';
import {GetSavedLaunchOptions, HideWindow, IsRunning, PickEIPBrowserProgram, ResumePendingConnect, SaveLaunchOptions, Start, Stop, SubmitInput} from '../wailsjs/go/main/App';

type LaunchOptions = main.LaunchOptions;
type LogEntry = {
  id: number;
  line: string;
};

const running = ref(false);
const statusMessage = ref('');
const logs = ref<LogEntry[]>([]);
const logContainer = ref<HTMLElement | null>(null);
const autoScroll = ref(true);

const fixedPort = 443;
const maxLogEntries = 1000;
const username = ref('');
const password = ref('');
const socksBind = ref('127.0.0.1:1080');
const httpBind = ref('127.0.0.1:8888');
const proxyOnlyMode = ref(false);
const debugDump = ref(false);
const activeTab = ref<'config' | 'logs'>('config');
const eipOptionsExpanded = ref(false);
const eipBrowserProgram = ref('');
const eipBrowserArgs = ref('');

const modalOpen = ref(false);
const modalTitle = ref('');
const modalPrompt = ref('');
const modalInput = ref('');
const modalInputKind = ref<'sms' | 'generic'>('generic');
const captchaBase64 = ref<string | null>(null);
const modalType = ref<'captcha' | 'input'>('input');
const captchaPoints = ref<Array<{ x: number; y: number }>>([]);
const captchaNaturalSize = ref<{ width: number; height: number } | null>(null);
const windowMaximised = ref(false);

const captchaSrc = computed(() => {
  if (!captchaBase64.value) {
    return '';
  }
  return `data:image/png;base64,${captchaBase64.value}`;
});

const isSmsPrompt = computed(() => modalType.value === 'input' && modalInputKind.value === 'sms');

let nextLogId = 0;

const extractCaptchaBase64 = (payload: unknown): string | null => {
  if (typeof payload === 'string') {
    const value = payload.trim();
    return value ? value : null;
  }

  if (Array.isArray(payload) && payload.length > 0) {
    return extractCaptchaBase64(payload[0]);
  }

  if (payload && typeof payload === 'object') {
    const data = payload as Record<string, unknown>;
    if (typeof data.base64 === 'string' && data.base64.trim()) {
      return data.base64;
    }
    if ('payload' in data) {
      return extractCaptchaBase64(data.payload);
    }
    if ('data' in data) {
      return extractCaptchaBase64(data.data);
    }
  }

  return null;
};

const appendLog = (line: string) => {
  logs.value.push({
    id: nextLogId,
    line,
  });
  nextLogId += 1;
  if (logs.value.length > maxLogEntries) {
    logs.value = logs.value.slice(-maxLogEntries);
  }
	if (autoScroll.value) {
	  nextTick(() => {
	    if (logContainer.value) {
	      logContainer.value.scrollTop = logContainer.value.scrollHeight;
	    }
	  });
	}
};

const closeModal = () => {
  modalOpen.value = false;
  modalInput.value = '';
  modalInputKind.value = 'generic';
  captchaBase64.value = null;
  captchaNaturalSize.value = null;
  clearCaptchaSelection();
};

const clearLogs = () => {
  logs.value = [];
};

const clearCaptchaSelection = () => {
  captchaPoints.value = [];
};

const removeLastCaptchaPoint = () => {
  captchaPoints.value = captchaPoints.value.slice(0, -1);
};

const openCaptchaModal = () => {
  modalType.value = 'captcha';
  modalTitle.value = '图形验证码';
  modalPrompt.value = '请在图片上按顺序点击对应位置，然后提交';
  modalInput.value = '';
  clearCaptchaSelection();
  modalOpen.value = true;
};

const openInputModal = (title: string, prompt: string, inputKind: 'sms' | 'generic' = 'generic') => {
  modalType.value = 'input';
  modalTitle.value = title;
  modalPrompt.value = prompt;
  modalInput.value = '';
  modalInputKind.value = inputKind;
  modalOpen.value = true;
};

const syncWindowMaximised = async () => {
  try {
    windowMaximised.value = await WindowIsMaximised();
  } catch {
    windowMaximised.value = false;
  }
};

const scheduleWindowMaximisedSync = () => {
  window.requestAnimationFrame(() => {
    void syncWindowMaximised();
  });
};

const handleWindowMinimise = () => {
  WindowMinimise();
};

const handleWindowToggleMaximise = () => {
  WindowToggleMaximise();
  windowMaximised.value = !windowMaximised.value;
  scheduleWindowMaximisedSync();
};

const handleWindowHide = async () => {
  try {
    await HideWindow();
  } catch (error) {
    statusMessage.value = (error as Error).message;
  }
};

const handleWindowResize = () => {
  scheduleWindowMaximisedSync();
};

const clamp = (value: number, min: number, max: number) => {
  if (value < min) {
    return min;
  }
  if (value > max) {
    return max;
  }
  return value;
};

const onCaptchaImageLoad = (event: Event) => {
  const img = event.target as HTMLImageElement;
  if (img.naturalWidth > 0 && img.naturalHeight > 0) {
    captchaNaturalSize.value = {
      width: img.naturalWidth,
      height: img.naturalHeight,
    };
  }
};

const onCaptchaImageClick = (event: MouseEvent) => {
  if (modalType.value !== 'captcha') {
    return;
  }

  const img = event.currentTarget as HTMLImageElement;
  const rect = img.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) {
    return;
  }

  const naturalWidth = img.naturalWidth;
  const naturalHeight = img.naturalHeight;
  if (naturalWidth <= 0 || naturalHeight <= 0) {
    return;
  }

  const x = clamp(Math.round(((event.clientX - rect.left) * naturalWidth) / rect.width), 0, naturalWidth - 1);
  const y = clamp(Math.round(((event.clientY - rect.top) * naturalHeight) / rect.height), 0, naturalHeight - 1);

  captchaNaturalSize.value = {
    width: naturalWidth,
    height: naturalHeight,
  };
  captchaPoints.value = [...captchaPoints.value, {x, y}];
};

const markerStyle = (point: { x: number; y: number }) => {
  const size = captchaNaturalSize.value;
  if (!size || size.width <= 1 || size.height <= 1) {
    return {
      left: '0%',
      top: '0%',
    };
  }

  return {
    left: `${(point.x / (size.width - 1)) * 100}%`,
    top: `${(point.y / (size.height - 1)) * 100}%`,
  };
};

const parseEIPBrowserArgs = (value: string): string[] => value
  .split(/\r?\n/)
  .map((line) => line.trim())
  .filter((line) => line.length > 0);

const currentLaunchOptions = (): LaunchOptions => ({
  protocol: 'atrust',
  server: 'sslvpn.scmcc.com.cn',
  port: fixedPort,
  username: username.value.trim(),
  password: password.value,
  socksBind: socksBind.value.trim(),
  httpBind: httpBind.value.trim(),
  secondaryDnsServer: '223.5.5.5',
  authType: 'auth/psw',
  loginDomain: 'AD',
  clientDataFile: 'client_data.json',
  eipBrowserProgram: eipBrowserProgram.value.trim(),
  eipBrowserArgs: parseEIPBrowserArgs(eipBrowserArgs.value),
  tunMode: !proxyOnlyMode.value,
  debugDump: debugDump.value,
});

const applySavedLaunchOptions = (options: LaunchOptions) => {
  username.value = options.username ?? '';
  password.value = options.password ?? '';
  socksBind.value = options.socksBind ?? '127.0.0.1:1080';
  httpBind.value = options.httpBind ?? '127.0.0.1:8888';
  eipBrowserProgram.value = options.eipBrowserProgram ?? '';
  eipBrowserArgs.value = (options.eipBrowserArgs ?? []).join('\n');
  proxyOnlyMode.value = !Boolean(options.tunMode);
  debugDump.value = Boolean(options.debugDump);
};

const buildLaunchOptions = (): LaunchOptions | null => {
  if (!username.value.trim()) {
    statusMessage.value = '用户名不能为空';
    return null;
  }
  if (!password.value) {
    statusMessage.value = '密码不能为空';
    return null;
  }
  if (!socksBind.value.trim()) {
    statusMessage.value = 'SOCKS 监听地址不能为空';
    return null;
  }
  if (!httpBind.value.trim()) {
    statusMessage.value = 'HTTP 监听地址不能为空';
    return null;
  }

  return currentLaunchOptions();
};

const handleSubmit = async () => {
  let payload = '';
  if (modalType.value === 'captcha') {
    if (captchaPoints.value.length === 0) {
      statusMessage.value = '请先点击验证码图片';
      return;
    }
    const size = captchaNaturalSize.value;
    if (!size || size.width <= 0 || size.height <= 0) {
      statusMessage.value = '验证码尺寸未就绪，请重新点击验证码';
      return;
    }

    payload = JSON.stringify({
      coordinates: captchaPoints.value.map((point) => [point.x, point.y]),
      width: size.width,
      height: size.height,
    });
  } else {
    if (!modalInput.value.trim()) {
      statusMessage.value = '请输入内容后再提交';
      return;
    }
    payload = modalInput.value.trim();
  }

  try {
    await SubmitInput(payload);
    statusMessage.value = '输入已提交';
    closeModal();
  } catch (error) {
    statusMessage.value = (error as Error).message;
  }
};

const handleSmsInputKeydown = (event: KeyboardEvent) => {
  if (event.key !== 'Enter' || event.isComposing) {
    return;
  }

  event.preventDefault();
  void handleSubmit();
};

const handleStart = async () => {
  statusMessage.value = '';
  const options = buildLaunchOptions();
  if (!options) {
    return;
  }

  try {
    await SaveLaunchOptions(options);
    await Start(options);
    statusMessage.value = '正在启动...';
  } catch (error) {
    statusMessage.value = (error as Error).message;
  }
};

const handleStop = async () => {
  statusMessage.value = '';
  try {
    await Stop();
    statusMessage.value = '已发送停止信号';
  } catch (error) {
    statusMessage.value = (error as Error).message;
  }
};

const handlePickEIPBrowserProgram = async () => {
  const currentValue = eipBrowserProgram.value;
  try {
    const selected = await PickEIPBrowserProgram();
    if (typeof selected === 'string' && selected.trim()) {
      eipBrowserProgram.value = selected;
      statusMessage.value = '已更新浏览器程序路径';
      return;
    }
  } catch (error) {
    statusMessage.value = error instanceof Error && error.message
      ? `选择浏览器程序失败：${error.message}`
      : '选择浏览器程序失败';
    eipBrowserProgram.value = currentValue;
    return;
  }

  eipBrowserProgram.value = currentValue;
};

const clearEIPBrowserProgram = () => {
  eipBrowserProgram.value = '';
};

let cleanupLog = () => {};
let cleanupState = () => {};
let cleanupCaptcha = () => {};
let cleanupInput = () => {};
let cleanupError = () => {};
let persistTimer: ReturnType<typeof setTimeout> | null = null;

const schedulePersist = () => {
  if (persistTimer) {
    clearTimeout(persistTimer);
  }
  persistTimer = setTimeout(() => {
    const options = currentLaunchOptions();
    void SaveLaunchOptions(options).catch(() => {
      // ignore autosave errors, startup path will surface persistent errors
    });
  }, 250);
};

onMounted(() => {
  cleanupLog = EventsOn('log', (line: string) => {
    appendLog(line);
  });

  cleanupState = EventsOn('state', (payload: { state?: string; running?: boolean; awaiting?: string; message?: string }) => {
    if (typeof payload.running === 'boolean') {
      running.value = payload.running;
    }
    if (payload.message) {
      statusMessage.value = payload.message;
    } else if (payload.awaiting) {
      statusMessage.value = `等待输入: ${payload.awaiting}`;
    } else if (payload.state === 'stopped' && !payload.running) {
      statusMessage.value = '已断开';
    }
  });

  cleanupCaptcha = EventsOn('need-captcha', (...args: unknown[]) => {
    const base64 = extractCaptchaBase64(args[0]);
    if (base64) {
      captchaBase64.value = base64;
      captchaNaturalSize.value = null;
      openCaptchaModal();
    }
  });

  cleanupInput = EventsOn('need-input', (payload: { type?: string; prompt?: string }) => {
    const prompt = payload?.prompt ?? '请输入内容';
    const inputKind = payload?.type === 'sms' ? 'sms' : 'generic';
    const title = inputKind === 'sms' ? '短信验证码' : '输入需求';
    openInputModal(title, prompt, inputKind);
  });

  cleanupError = EventsOn('error', (message: string) => {
    statusMessage.value = message;
  });

  void IsRunning()
    .then((value) => {
      running.value = value;
    })
    .catch(() => {
      running.value = false;
    });

  void GetSavedLaunchOptions()
    .then((options) => {
      applySavedLaunchOptions(options);
    })
    .catch(() => {
      // keep defaults when no persisted settings are available
    });

  void ResumePendingConnect()
    .then((resumed) => {
      if (resumed) {
        statusMessage.value = '已切换到管理员模式，正在恢复连接...';
      }
    })
    .catch((error) => {
      statusMessage.value = (error as Error).message;
    });

  void syncWindowMaximised();
  window.addEventListener('resize', handleWindowResize);
});

watch([username, password, socksBind, httpBind, eipBrowserProgram, eipBrowserArgs, proxyOnlyMode, debugDump], () => {
  schedulePersist();
});

onUnmounted(() => {
  cleanupLog();
  cleanupState();
  cleanupCaptcha();
  cleanupInput();
  cleanupError();
  window.removeEventListener('resize', handleWindowResize);
  if (persistTimer) {
    clearTimeout(persistTimer);
    persistTimer = null;
  }
});
</script>

<template>
  <div class="app-shell">
    <div class="app-titlebar-shell">
      <div class="panel-surface app-titlebar" @dblclick="handleWindowToggleMaximise">
        <div class="app-titlebar__brand">
          <span class="app-titlebar__mark" aria-hidden="true"></span>
          <span class="app-titlebar__label">ZJU Connect GUI</span>
        </div>
        <div class="app-titlebar__controls" aria-label="窗口控制" @dblclick.stop>
          <button
            type="button"
            class="window-control"
            aria-label="最小化窗口"
            @click="handleWindowMinimise"
          >
            <span class="window-control__glyph window-control__glyph--minimise" aria-hidden="true"></span>
          </button>
          <button
            type="button"
            class="window-control"
            :aria-label="windowMaximised ? '还原窗口' : '最大化窗口'"
            @click="handleWindowToggleMaximise"
          >
            <span
              class="window-control__glyph"
              :class="windowMaximised ? 'window-control__glyph--restore' : 'window-control__glyph--maximise'"
              aria-hidden="true"
            ></span>
          </button>
          <button
            type="button"
            class="window-control window-control--close"
            aria-label="隐藏窗口到托盘"
            @click="handleWindowHide"
          >
            <span class="window-control__glyph window-control__glyph--close" aria-hidden="true"></span>
          </button>
        </div>
      </div>
    </div>

    <div class="app-frame">

      <header class="panel-surface app-header">
        <div class="app-header__bar">
          <div class="app-heading">
            <h1 class="app-title">ZJU Connect GUI</h1>
            <p class="app-subtitle">基于 zju-connect CLI 的桌面控制台</p>
          </div>
        </div>
        <p v-if="statusMessage" class="status-banner">{{ statusMessage }}</p>
      </header>

      <main class="app-main">
        <div class="tab-switcher">
          <button
            class="tab-switcher__button"
            :class="{ 'is-active': activeTab === 'config' }"
            @click="activeTab = 'config'"
          >
            配置
          </button>
          <button
            class="tab-switcher__button"
            :class="{ 'is-active': activeTab === 'logs' }"
            @click="activeTab = 'logs'"
          >
            日志
          </button>
        </div>

        <section v-if="activeTab === 'config'" class="section-stack">
          <div class="section-heading">
            <h2 class="section-title">连接设置</h2>
            <p class="section-copy">按需填写账号、本地代理监听地址和 EIP 打开方式即可。</p>
          </div>

          <div class="panel-surface settings-card">
            <div class="info-banner">
              <p class="info-banner__title">仅代理模式说明</p>
              <p class="info-banner__copy">开启后，只提供本机 SOCKS 和 HTTP 代理端口，适合手动给浏览器或其他应用填写代理。</p>
              <p class="info-banner__copy">关闭后，除了本地代理端口，还会额外启用 TUN / 系统路由接管，让系统流量按连接规则走。</p>
            </div>

            <div class="settings-group">
              <div class="settings-group__header">
                <h3 class="settings-group__title">账号认证</h3>
                <p class="settings-group__copy">使用校园统一认证信息发起连接。</p>
              </div>
              <div class="field-grid">
                <label class="field">
                  <span class="field__label">用户名</span>
                  <input
                    v-model="username"
                    type="text"
                    class="text-field"
                  />
                </label>

                <label class="field">
                  <span class="field__label">密码</span>
                  <input
                    v-model="password"
                    type="password"
                    class="text-field"
                  />
                </label>
              </div>
            </div>

            <div class="settings-divider"></div>

            <div class="settings-group">
              <div class="settings-group__header">
                <h3 class="settings-group__title">本地代理</h3>
                <p class="settings-group__copy">保持 SOCKS 与 HTTP 两组监听地址，供本机应用接入。</p>
              </div>
              <div class="field-grid">
                <label class="field">
                  <span class="field__label">SOCKS 监听地址</span>
                  <input
                    v-model="socksBind"
                    type="text"
                    class="text-field"
                  />
                </label>

                <label class="field">
                  <span class="field__label">HTTP 监听地址</span>
                  <input
                    v-model="httpBind"
                    type="text"
                    class="text-field"
                  />
                </label>
              </div>
            </div>

            <div class="settings-divider"></div>

            <div class="settings-group">
              <div class="settings-group__header">
                <h3 class="settings-group__title">运行方式</h3>
                <p class="settings-group__copy">调整连接模式与诊断输出，不改变现有控制逻辑。</p>
              </div>
              <div class="toggle-grid">
                <label class="toggle-card">
                  <div class="toggle-card__row">
                    <div class="toggle-card__content">
                      <span class="toggle-card__title">仅代理模式</span>
                      <p class="toggle-card__copy">开启后仅保留本地 SOCKS / HTTP 代理；关闭后还会启用 TUN 与系统路由。</p>
                    </div>
                    <input v-model="proxyOnlyMode" type="checkbox" class="check-input" />
                  </div>
                </label>

                <label class="toggle-card">
                  <div class="toggle-card__row">
                    <div class="toggle-card__content">
                      <span class="toggle-card__title">调试模式</span>
                      <p class="toggle-card__copy">保留更详细的诊断输出，便于排查连接问题。</p>
                    </div>
                    <input v-model="debugDump" type="checkbox" class="check-input" />
                  </div>
                </label>
              </div>
            </div>

            <div class="settings-divider"></div>

            <details class="advanced-card" :open="eipOptionsExpanded">
              <summary
                class="advanced-card__summary"
                @click.prevent="eipOptionsExpanded = !eipOptionsExpanded"
              >
                <div class="advanced-card__heading">
                  <span class="advanced-card__title">EIP 打开设置</span>
                  <p class="advanced-card__copy">默认折叠，可按需指定浏览器程序和附加参数。</p>
                </div>
                <span class="advanced-card__hint">{{ eipOptionsExpanded ? '收起' : '展开' }}</span>
              </summary>
              <div v-if="eipOptionsExpanded" class="advanced-card__content">
                <label class="field">
                  <span class="field__label">浏览器程序路径</span>
                  <div class="inline-field-row">
                    <input
                      v-model="eipBrowserProgram"
                      type="text"
                      class="text-field"
                      placeholder="留空则使用系统默认浏览器"
                    />
                    <div class="inline-actions">
                      <button
                        type="button"
                        class="app-button app-button--secondary"
                        @click="handlePickEIPBrowserProgram"
                      >
                        浏览...
                      </button>
                      <button
                        v-if="eipBrowserProgram"
                        type="button"
                        class="app-button app-button--ghost"
                        @click="clearEIPBrowserProgram"
                      >
                        清空
                      </button>
                    </div>
                  </div>
                </label>

                <label class="field">
                  <span class="field__label">浏览器参数（每行一个）</span>
                  <textarea
                    v-model="eipBrowserArgs"
                    class="text-area"
                    placeholder="每行一个参数，启动时会自动在最后追加 URL"
                  ></textarea>
                </label>
              </div>
            </details>
          </div>
        </section>

        <section v-else class="section-stack">
          <div class="section-heading section-heading--split">
            <div>
              <h2 class="section-title">运行日志</h2>
              <p class="section-copy">查看当前会话输出，保留自动滚动与清空控制。</p>
            </div>
            <div class="section-actions">
              <label class="toggle-chip">
                <input v-model="autoScroll" type="checkbox" class="check-input" />
                <span>自动滚动</span>
              </label>
              <button class="app-button app-button--secondary app-button--small" @click="clearLogs">
                清空
              </button>
            </div>
          </div>

          <div ref="logContainer" class="panel-surface log-panel">
            <div v-if="logs.length === 0" class="log-empty">暂无日志输出</div>
            <ol v-else class="log-list">
              <li v-for="(entry, index) in logs" :key="entry.id" class="log-row">
                <span class="log-row__index">{{ index + 1 }}</span>
                <span class="log-row__text">{{ entry.line }}</span>
              </li>
            </ol>
          </div>
        </section>
      </main>
    </div>

    <button
      v-if="!running"
      type="button"
      class="app-button app-button--primary settings-fab"
      aria-label="开始连接"
      @click="handleStart"
    >
      <span class="settings-fab__icon settings-fab__icon--start" aria-hidden="true"></span>
    </button>
    <button
      v-else
      type="button"
      class="app-button app-button--danger settings-fab"
      aria-label="断开连接"
      @click="handleStop"
    >
      <span class="settings-fab__icon settings-fab__icon--stop" aria-hidden="true"></span>
    </button>

    <div
      v-if="modalOpen"
      class="modal-backdrop"
    >
      <div class="panel-surface modal-card" :class="{ 'modal-card--compact': isSmsPrompt }">
        <h3 class="modal-title">{{ modalTitle }}</h3>
        <p class="modal-copy">{{ modalPrompt }}</p>
        <div v-if="modalType === 'captcha'" class="modal-body">
          <div class="captcha-frame">
            <img
              v-if="captchaBase64"
              :src="captchaSrc"
              alt="captcha"
              class="captcha-image"
              @load="onCaptchaImageLoad"
              @click="onCaptchaImageClick"
            />
            <div
              v-for="(point, index) in captchaPoints"
              :key="`${point.x}-${point.y}-${index}`"
              class="captcha-marker"
              :style="markerStyle(point)"
            >
              <div class="captcha-marker__dot">
                {{ index + 1 }}
              </div>
            </div>
          </div>

          <div class="modal-inline-actions">
            <button class="app-button app-button--secondary app-button--small" @click="removeLastCaptchaPoint">撤销上一步</button>
            <button class="app-button app-button--ghost app-button--small" @click="clearCaptchaSelection">清空坐标</button>
            <span class="points-chip">已选择 {{ captchaPoints.length }} 个点</span>
          </div>

          <div class="coordinate-box">
            <span v-if="captchaPoints.length === 0">尚未选择坐标</span>
            <span v-else>{{ JSON.stringify(captchaPoints.map((point) => [point.x, point.y])) }}</span>
          </div>
        </div>

        <input
          v-else-if="isSmsPrompt"
          v-model="modalInput"
          type="text"
          autocomplete="one-time-code"
          spellcheck="false"
          class="text-field text-field--modal-compact"
          placeholder="请输入短信验证码"
          @keydown="handleSmsInputKeydown"
        />

        <textarea
          v-else
          v-model="modalInput"
          class="text-area text-area--modal"
          placeholder="请输入响应内容"
        ></textarea>
        <div class="modal-footer">
          <button class="app-button app-button--ghost" @click="closeModal">取消</button>
          <button class="app-button app-button--primary" @click="handleSubmit">提交</button>
        </div>
      </div>
    </div>
  </div>
</template>
