<script lang="ts" setup>
import {computed, nextTick, onMounted, onUnmounted, ref} from 'vue';
import {EventsOn} from '../wailsjs/runtime/runtime';
import {IsRunning, Start, Stop, SubmitInput} from '../wailsjs/go/main/App';
import type {LaunchOptions} from '../wailsjs/go/main/App';

const running = ref(false);
const statusMessage = ref('');
const logs = ref<string[]>([]);
const logContainer = ref<HTMLElement | null>(null);
const autoScroll = ref(true);

const port = ref(443);
const username = ref('');
const password = ref('');
const socksBind = ref('127.0.0.1:1080');
const httpBind = ref('127.0.0.1:8888');
const tunMode = ref(false);
const debugDump = ref(false);

const modalOpen = ref(false);
const modalTitle = ref('');
const modalPrompt = ref('');
const modalInput = ref('');
const captchaBase64 = ref<string | null>(null);

const captchaSrc = computed(() => {
  if (!captchaBase64.value) {
    return '';
  }
  return `data:image/png;base64,${captchaBase64.value}`;
});

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
};

const openModal = (title: string, prompt: string) => {
  modalTitle.value = title;
  modalPrompt.value = prompt;
  modalInput.value = '';
  modalOpen.value = true;
};

const closeModal = () => {
  modalOpen.value = false;
  modalInput.value = '';
  captchaBase64.value = null;
};

const clearLogs = () => {
  logs.value = [];
};

const buildLaunchOptions = (): LaunchOptions | null => {
  if (port.value < 1 || port.value > 65535) {
    statusMessage.value = '端口范围必须是 1-65535';
    return null;
  }
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

  return {
    protocol: 'atrust',
    server: 'sslvpn.scmcc.com.cn',
    port: port.value,
    username: username.value.trim(),
    password: password.value,
    socksBind: socksBind.value.trim(),
    httpBind: httpBind.value.trim(),
    secondaryDnsServer: '223.5.5.5',
    authType: 'auth/psw',
    loginDomain: 'AD',
    clientDataFile: 'client_data.json',
    tunMode: tunMode.value,
    debugDump: debugDump.value,
  };
};

const handleSubmit = async () => {
  if (!modalInput.value.trim()) {
    statusMessage.value = '请输入内容后再提交';
    return;
  }
  try {
    await SubmitInput(modalInput.value.trim());
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

onMounted(async () => {
  running.value = await IsRunning();

  cleanupLog = EventsOn('log', (line: string) => {
    appendLog(line);
  });

  cleanupState = EventsOn('state', (payload: { running?: boolean; awaiting?: string }) => {
    if (typeof payload.running === 'boolean') {
      running.value = payload.running;
    }
    if (payload.awaiting) {
      statusMessage.value = `等待输入: ${payload.awaiting}`;
    }
  });

  cleanupCaptcha = EventsOn('need-captcha', (payload: { base64?: string } | string | undefined) => {
    const base64 = typeof payload === 'string' ? payload : payload?.base64;
    if (!base64) {
      return;
    }
    captchaBase64.value = base64;
    openModal('验证码输入', '请输入验证码 JSON');
  });

  cleanupInput = EventsOn('need-input', (payload: { type?: string; prompt?: string }) => {
    const prompt = payload?.prompt ?? '请输入内容';
    const title = payload?.type === 'sms' ? '短信验证码' : '输入需求';
    openModal(title, prompt);
  });

  cleanupError = EventsOn('error', (message: string) => {
    statusMessage.value = message;
  });
});

onUnmounted(() => {
  cleanupLog();
  cleanupState();
  cleanupCaptcha();
  cleanupInput();
  cleanupError();
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

    <main class="grid gap-6 px-6 py-6 lg:grid-cols-[1.2fr_1fr]">
      <section class="space-y-4">
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
        <div
          ref="logContainer"
          class="h-[420px] overflow-auto rounded-xl border border-slate-300 bg-white p-4 font-mono text-xs leading-relaxed text-slate-700"
        >
          <div v-if="logs.length === 0" class="text-slate-400">暂无日志输出</div>
          <div v-for="(line, index) in logs" :key="index" class="whitespace-pre-wrap">
            {{ line }}
          </div>
        </div>
      </section>

      <section class="space-y-4">
        <div>
          <h2 class="text-lg font-semibold">启动参数</h2>
          <p class="text-xs text-slate-500">仅保留界面可配置项，其余参数按预设值自动携带</p>
        </div>

        <div class="space-y-4 rounded-xl border border-slate-300 bg-white p-4">
          <label class="block text-sm">
            <span class="mb-1 block text-slate-600">端口</span>
            <input
              v-model.number="port"
              type="number"
              min="1"
              max="65535"
              class="w-full rounded-lg border border-slate-300 px-3 py-2 text-slate-800 outline-none focus:border-emerald-500"
            />
          </label>

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
            <label class="flex items-center justify-between rounded-lg border border-slate-300 px-3 py-2 text-sm text-slate-700">
              <span>TUN 模式</span>
              <input v-model="tunMode" type="checkbox" class="h-4 w-4 rounded border-slate-400" />
            </label>

            <label class="flex items-center justify-between rounded-lg border border-slate-300 px-3 py-2 text-sm text-slate-700">
              <span>调试模式</span>
              <input v-model="debugDump" type="checkbox" class="h-4 w-4 rounded border-slate-400" />
            </label>
          </div>

          <div class="rounded-lg bg-slate-100 p-3 text-xs text-slate-600">
            <p class="font-semibold text-slate-700">界面隐藏固定参数</p>
            <ul class="mt-2 list-inside list-disc space-y-1">
              <li>-protocol atrust</li>
              <li>-server sslvpn.scmcc.com.cn</li>
              <li>-disable-zju-config</li>
              <li>-secondary-dns-server 223.5.5.5</li>
              <li>-auth-type auth/psw</li>
              <li>-login-domain AD</li>
              <li>-client-data-file client_data.json</li>
              <li>TUN 开启时附加：-tun-mode -add-route -dns-hijack -fake-ip</li>
              <li>调试开关开启时附加：-debug-dump</li>
            </ul>
          </div>
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
        <img
          v-if="captchaBase64"
          :src="captchaSrc"
          alt="captcha"
          class="mt-4 w-full rounded-lg border border-slate-300"
        />
        <textarea
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
