<script setup lang="ts">
import { ref } from "vue";
import { useAssistantStore } from "../../../core/stores/assistant";
import type { AppSettings } from "../../../core/stores/settings";
import SettingsCard from "../../../components/settings/SettingsCard.vue";
import SettingsTextField from "../../../components/settings/SettingsTextField.vue";
import AvatarCropper from "../../../components/ui/AvatarCropper.vue";
import VcpAvatar from "../../../components/ui/VcpAvatar.vue";

defineProps<{
  settings: AppSettings;
}>();

const assistantStore = useAssistantStore();

// Avatar Logic
const fileInput = ref<HTMLInputElement | null>(null);
const isCropping = ref(false);
const cropImg = ref("");
const avatarVersion = ref(0);

const triggerFileInput = () => {
  fileInput.value?.click();
};

const handleFileChange = (e: Event) => {
  const file = (e.target as HTMLInputElement).files?.[0];
  if (!file) return;

  const reader = new FileReader();
  reader.onload = (event) => {
    cropImg.value = event.target?.result as string;
    isCropping.value = true;
  };
  reader.readAsDataURL(file);
};

// Removed avatarUrl computed as we use avatarDisplayUrl via IPC

const onCropConfirm = async (blob: Blob) => {
  isCropping.value = false;
  
  try {
    const arrayBuffer = await blob.arrayBuffer();
    const bytes = new Uint8Array(arrayBuffer);
    
    // Use assistantStore to save avatar and get notification
    await assistantStore.saveAvatar("user", "user_avatar", blob.type, Array.from(bytes));

    // Refresh avatar by updating timestamp
    avatarVersion.value = Date.now();
    console.log("Avatar updated, triggering reload");
  } catch (err) {
    console.error("Failed to save user avatar:", err);
  }
};
</script>

<template>
  <SettingsCard variant="glass">
    <div class="flex items-center gap-5">
      <div @click="triggerFileInput" class="group cursor-pointer active:scale-95 transition-all relative">
        <VcpAvatar 
          owner-type="user" 
          owner-id="user_avatar" 
          :version="avatarVersion"
          :fallback-name="settings.userName"
          size="w-16 h-16"
          rounded="rounded-2xl"
        />
        <div class="absolute inset-0 rounded-2xl bg-black/40 opacity-0 group-hover:opacity-100 flex items-center justify-center z-20 transition-opacity">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M23 19a2 2 0 0 1-2 2H3a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4l2-3h6l2 3h4a2 2 0 0 1 2 2z"></path><circle cx="12" cy="13" r="4"></circle></svg>
        </div>
      </div>
      <div class="flex-1 min-w-0">
        <SettingsTextField v-model="settings.userName" label="用户名" placeholder="输入你的名字..." />
      </div>
    </div>
    
    <div class="mt-4 pt-4 border-t border-black/5 dark:border-white/5 space-y-4">
      <div class="flex gap-4">
        <div class="flex-1">
          <SettingsTextField 
            v-model="settings.adminUsername" 
            label="管理员账号" 
            placeholder="VCP 管理员用户名" 
            mono
          />
        </div>
        <div class="flex-1">
          <SettingsTextField 
            v-model="settings.adminPassword" 
            label="管理员密码" 
            placeholder="鉴权密码" 
            is-secure 
            mono
          />
        </div>
      </div>
      <p class="text-[10px] opacity-40 px-1 italic">
        * 用于远程获取表情包库等管理接口鉴权 (Basic Auth)
      </p>
    </div>

    <input type="file" ref="fileInput" class="hidden" accept="image/*" @change="handleFileChange" />
  </SettingsCard>

  <!-- 头像裁剪器 -->
  <AvatarCropper v-if="isCropping" :img="cropImg" @cancel="isCropping = false" @confirm="onCropConfirm" />
</template>
