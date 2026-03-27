<script lang="ts" setup>
import {computed, nextTick, onMounted, onUnmounted, ref, watch} from 'vue';
import {EventsOn} from '../wailsjs/runtime/runtime';
import type {main} from '../wailsjs/go/models';
import {GetSavedLaunchOptions, IsRunning, ResumePendingConnect, SaveLaunchOptions, Start, Stop, SubmitInput} from '../wailsjs/go/main/App';

type LaunchOptions = main.LaunchOptions;

const running = ref(false);
const statusMessage = ref('');
const logs = ref<string[]>([]);
const logContainer = ref<HTMLElement | null>(null);
const autoScroll = ref(true);

const fixedPort = 443;
const username = ref('');
const password = ref('');
const socksBind = ref('127.0.0.1:1080');
const httpBind = ref('127.0.0.1:8888');
const proxyOnlyMode = ref(false);
const debugDump = ref(false);
const activeTab = ref<'config' | 'logs'>('config');
const fixedOptionsExpanded = ref(false);
const eipOptionsExpanded = ref(false);
const eipBrowserProgram = ref('');
const eipBrowserArgs = ref('');

const modalOpen = ref(false);
const modalTitle = ref('');
const modalPrompt = ref('');
const modalInput = ref('');
const captchaBase64 = ref<string | null>(null);
const modalType = ref<'captcha' | 'input'>('input');
const captchaPoints = ref<Array<{ x: number; y: number }>>([]);
const captchaNaturalSize = ref<{ width: number; height: number } | null>(null);

const captchaSrc = computed(() => {
  if (!captchaBase64.value) {
    return '';
  }
  return `data:image/png;base64,${captchaBase64.value}`;
});

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
  logs.value.push(line);
  if (logs.value.length > 1000) {
    logs.value.shift();
  }
  if (autoScroll.value) {
    nextTick(() => {
      if (logContainer.value) {
        logContainer.value.scrollTop = logContainer.value.scrollHeight;
      }
    });
  }

  if (line.includes('VPN client started')) {
    statusMessage.value = '已启动';
  }
};

const closeModal = () => {
  modalOpen.value = false;
  modalInput.value = '';
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

const openInputModal = (title: string, prompt: string) => {
  modalType.value = 'input';
  modalTitle.value = title;
  modalPrompt.value = prompt;
  modalInput.value = '';
  modalOpen.value = true;
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
    const title = payload?.type === 'sms' ? '短信验证码' : '输入需求';
    openInputModal(title, prompt);
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
  if (persistTimer) {
    clearTimeout(persistTimer);
    persistTimer = null;
  }
});
</script>

<template>
  <div class="min-h-full bg-[#F2F2F2] text-slate-800">
    <header class="border-b border-slate-300 bg-white px-6 py-4 shadow-sm">
      <div class="flex items-center justify-between">
        <div>
          <h1 class="text-xl font-semibold">ZJU Connect GUI</h1>
          <p class="text-sm text-slate-500">基于 zju-connect CLI 的桌面控制台</p>
        </div>
        <button
          v-if="!running"
          class="rounded-lg bg-emerald-500 px-4 py-2 text-sm font-semibold text-white hover:bg-emerald-600"
          @click="handleStart"
        >
          连接
        </button>
        <button
          v-else
          class="rounded-lg bg-rose-500 px-4 py-2 text-sm font-semibold text-white hover:bg-rose-600"
          @click="handleStop"
        >
          断开
        </button>
      </div>
      <p v-if="statusMessage" class="mt-2 text-sm text-amber-700">{{ statusMessage }}</p>
    </header>

    <main class="px-6 py-6">
      <div class="mb-4 inline-flex rounded-xl border border-slate-300 bg-white p-1 shadow-sm">
        <button
          class="rounded-lg px-4 py-2 text-sm font-semibold"
          :class="activeTab === 'config' ? 'bg-emerald-500 text-white' : 'text-slate-600 hover:bg-slate-100'"
          @click="activeTab = 'config'"
        >
          配置
        </button>
        <button
          class="rounded-lg px-4 py-2 text-sm font-semibold"
          :class="activeTab === 'logs' ? 'bg-emerald-500 text-white' : 'text-slate-600 hover:bg-slate-100'"
          @click="activeTab = 'logs'"
        >
          日志
        </button>
      </div>

      <section v-if="activeTab === 'config'" class="space-y-4">
        <div>
          <h2 class="text-lg font-semibold">启动参数</h2>
          <p class="text-xs text-slate-500">仅保留界面可配置项，其余参数按预设值自动携带</p>
        </div>

        <div class="space-y-4 rounded-xl border border-slate-300 bg-white p-4">
          <label class="block text-sm">
            <span class="mb-1 block text-slate-600">用户名</span>
            <input
              v-model="username"
              type="text"
              class="w-full rounded-lg border border-slate-300 px-3 py-2 text-slate-800 outline-none focus:border-emerald-500"
            />
          </label>

          <label class="block text-sm">
            <span class="mb-1 block text-slate-600">密码</span>
            <input
              v-model="password"
              type="password"
              class="w-full rounded-lg border border-slate-300 px-3 py-2 text-slate-800 outline-none focus:border-emerald-500"
            />
          </label>

          <label class="block text-sm">
            <span class="mb-1 block text-slate-600">SOCKS 监听地址</span>
            <input
              v-model="socksBind"
              type="text"
              class="w-full rounded-lg border border-slate-300 px-3 py-2 text-slate-800 outline-none focus:border-emerald-500"
            />
          </label>

          <label class="block text-sm">
            <span class="mb-1 block text-slate-600">HTTP 监听地址</span>
            <input
              v-model="httpBind"
              type="text"
              class="w-full rounded-lg border border-slate-300 px-3 py-2 text-slate-800 outline-none focus:border-emerald-500"
            />
          </label>

          <div class="grid gap-3 sm:grid-cols-2">
            <label class="rounded-lg border border-slate-300 px-3 py-2 text-sm text-slate-700">
              <div class="flex items-center justify-between gap-3">
                <span>仅代理模式</span>
                <input v-model="proxyOnlyMode" type="checkbox" class="h-4 w-4 rounded border-slate-400" />
              </div>
              <p class="mt-1 text-xs text-slate-500">如果不知道是什么，请不要打开。</p>
            </label>

            <label class="flex items-center justify-between rounded-lg border border-slate-300 px-3 py-2 text-sm text-slate-700">
              <span>调试模式</span>
              <input v-model="debugDump" type="checkbox" class="h-4 w-4 rounded border-slate-400" />
            </label>
          </div>

          <details class="rounded-lg bg-slate-100 p-3 text-xs text-slate-600" :open="eipOptionsExpanded">
            <summary
              class="cursor-pointer select-none font-semibold text-slate-700"
              @click.prevent="eipOptionsExpanded = !eipOptionsExpanded"
            >
              EIP 打开设置（默认折叠）
            </summary>
            <div v-if="eipOptionsExpanded" class="mt-3 space-y-3">
              <label class="block text-sm">
                <span class="mb-1 block text-slate-600">浏览器程序路径</span>
                <input
                  v-model="eipBrowserProgram"
                  type="text"
                  class="w-full rounded-lg border border-slate-300 bg-white px-3 py-2 text-slate-800 outline-none focus:border-emerald-500"
                  placeholder="留空则使用系统默认浏览器"
                />
              </label>

              <label class="block text-sm">
                <span class="mb-1 block text-slate-600">浏览器参数（每行一个）</span>
                <textarea
                  v-model="eipBrowserArgs"
                  class="h-28 w-full rounded-lg border border-slate-300 bg-white p-3 text-slate-800 outline-none focus:border-emerald-500"
                  placeholder="每行一个参数，启动时会自动在最后追加 URL"
                ></textarea>
              </label>
            </div>
          </details>

          <details class="rounded-lg bg-slate-100 p-3 text-xs text-slate-600" :open="fixedOptionsExpanded">
            <summary
              class="cursor-pointer select-none font-semibold text-slate-700"
              @click.prevent="fixedOptionsExpanded = !fixedOptionsExpanded"
            >
              固定参数（默认折叠）
            </summary>
            <ul v-if="fixedOptionsExpanded" class="mt-2 list-inside list-disc space-y-1">
              <li>-protocol atrust</li>
              <li>-server sslvpn.scmcc.com.cn</li>
              <li>-port 443</li>
              <li>-disable-zju-config</li>
              <li>-secondary-dns-server 223.5.5.5</li>
              <li>-auth-type auth/psw</li>
              <li>-login-domain AD</li>
              <li>-client-data-file client_data.json</li>
              <li>仅代理模式关闭时附加：-tun-mode -add-route -dns-hijack -fake-ip</li>
              <li>调试开关开启时附加：-debug-dump</li>
            </ul>
          </details>
        </div>
      </section>

      <section v-else class="space-y-4">
        <div class="flex items-center justify-between">
          <h2 class="text-lg font-semibold">运行日志</h2>
          <div class="flex items-center gap-3 text-sm text-slate-600">
            <label class="flex items-center gap-2">
              <input v-model="autoScroll" type="checkbox" class="h-4 w-4 rounded border-slate-400" />
              自动滚动
            </label>
            <button class="rounded border border-slate-300 bg-white px-3 py-1 hover:bg-slate-100" @click="clearLogs">
              清空
            </button>
          </div>
        </div>

        <div ref="logContainer" class="h-[520px] overflow-auto rounded-xl border border-slate-300 bg-white">
          <div v-if="logs.length === 0" class="p-4 text-sm text-slate-400">暂无日志输出</div>
          <ol v-else class="divide-y divide-slate-200 font-mono text-xs text-slate-700">
            <li v-for="(line, index) in logs" :key="index" class="flex gap-3 px-4 py-2 leading-relaxed">
              <span class="w-10 shrink-0 text-slate-400">{{ index + 1 }}</span>
              <span class="whitespace-pre-wrap break-all">{{ line }}</span>
            </li>
          </ol>
        </div>
      </section>
    </main>

    <div
      v-if="modalOpen"
      class="fixed inset-0 z-50 flex items-center justify-center bg-black/35 px-4"
    >
      <div class="w-full max-w-lg rounded-xl bg-white p-6 shadow-xl">
        <h3 class="text-lg font-semibold">{{ modalTitle }}</h3>
        <p class="mt-2 text-sm text-slate-600">{{ modalPrompt }}</p>
        <div v-if="modalType === 'captcha'" class="mt-4 space-y-3">
          <div class="relative overflow-hidden rounded-lg border border-slate-300 bg-slate-100">
            <img
              v-if="captchaBase64"
              :src="captchaSrc"
              alt="captcha"
              class="block w-full cursor-crosshair"
              @load="onCaptchaImageLoad"
              @click="onCaptchaImageClick"
            />
            <div
              v-for="(point, index) in captchaPoints"
              :key="`${point.x}-${point.y}-${index}`"
              class="pointer-events-none absolute -translate-x-1/2 -translate-y-1/2"
              :style="markerStyle(point)"
            >
              <div class="flex h-6 w-6 items-center justify-center rounded-full bg-emerald-600 text-xs font-bold text-white shadow">
                {{ index + 1 }}
              </div>
            </div>
          </div>

          <div class="flex items-center gap-2 text-sm">
            <button class="rounded border border-slate-300 bg-white px-3 py-1 hover:bg-slate-100" @click="removeLastCaptchaPoint">撤销上一步</button>
            <button class="rounded border border-slate-300 bg-white px-3 py-1 hover:bg-slate-100" @click="clearCaptchaSelection">清空坐标</button>
            <span class="text-slate-500">已选择 {{ captchaPoints.length }} 个点</span>
          </div>

          <div class="max-h-24 overflow-auto rounded border border-slate-200 bg-slate-50 p-2 text-xs text-slate-600">
            <span v-if="captchaPoints.length === 0">尚未选择坐标</span>
            <span v-else>{{ JSON.stringify(captchaPoints.map((point) => [point.x, point.y])) }}</span>
          </div>
        </div>

        <textarea
          v-else
          v-model="modalInput"
          class="mt-4 h-28 w-full rounded-lg border border-slate-300 bg-white p-3 text-sm text-slate-700"
          placeholder="请输入响应内容"
        ></textarea>
        <div class="mt-4 flex justify-end gap-2">
          <button class="rounded border border-slate-300 bg-white px-4 py-2 hover:bg-slate-100" @click="closeModal">取消</button>
          <button class="rounded bg-emerald-500 px-4 py-2 text-white hover:bg-emerald-600" @click="handleSubmit">提交</button>
        </div>
      </div>
    </div>
  </div>
</template>
