<script setup lang="ts">
import { onUnmounted, ref } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import { deleteTempFile, pickFile } from "tauri-plugin-vcp-mobile";
import { useAssistantStore } from "../../../core/stores/assistant";
import { useNotificationStore } from "../../../core/stores/notification";
import type { AppSettings } from "../../../core/stores/settings";
import SettingsCard from "../../../components/settings/SettingsCard.vue";
import SettingsTextField from "../../../components/settings/SettingsTextField.vue";
import AvatarCropper from "../../../components/ui/AvatarCropper.vue";
import VcpAvatar from "../../../components/ui/VcpAvatar.vue";

defineProps<{
  settings: AppSettings;
}>();

const assistantStore = useAssistantStore();
const notificationStore = useNotificationStore();

// Avatar Logic
const isCropping = ref(false);
const isPickingAvatar = ref(false);
const cropImg = ref("");
const avatarWorkingPath = ref<string | null>(null);
let disposed = false;

const releaseAvatarWorkingCopy = async () => {
  const filePath = avatarWorkingPath.value;
  avatarWorkingPath.value = null;
  cropImg.value = "";
  if (!filePath) return;
  try {
    await deleteTempFile(filePath);
  } catch (error) {
    console.warn("Failed to delete user avatar working copy:", error);
  }
};

const triggerFileInput = async () => {
  if (isPickingAvatar.value || isCropping.value) return;
  isPickingAvatar.value = true;
  try {
    await releaseAvatarWorkingCopy();
    const picked = await pickFile("avatar");
    if (disposed) {
      await deleteTempFile(picked.path);
      return;
    }
    avatarWorkingPath.value = picked.path;
    cropImg.value = convertFileSrc(picked.path);
    isCropping.value = true;
  } catch (error) {
    if (!String(error).toLowerCase().includes("cancel")) {
      console.error("Failed to prepare user avatar image:", error);
      notificationStore.addNotification({
        type: "error",
        title: "头像读取失败",
        message: String(error) || "请选择其他图片后重试",
        toastOnly: true,
      });
    }
  } finally {
    if (!disposed) isPickingAvatar.value = false;
  }
};

const cancelAvatarCrop = () => {
  isCropping.value = false;
  void releaseAvatarWorkingCopy();
};

// Removed avatarUrl computed as we use avatarDisplayUrl via IPC

const onCropConfirm = async (blob: Blob) => {
  isCropping.value = false;
  await releaseAvatarWorkingCopy();
  
  try {
    const arrayBuffer = await blob.arrayBuffer();
    const bytes = new Uint8Array(arrayBuffer);
    
    // Use assistantStore to save avatar and get notification
    await assistantStore.saveAvatar("user", "user_avatar", blob.type, bytes);

  } catch (err) {
    console.error("Failed to save user avatar:", err);
  }
};

onUnmounted(() => {
  disposed = true;
  void releaseAvatarWorkingCopy();
});
</script>

<template>
  <SettingsCard variant="glass">
    <div class="flex items-center gap-5">
      <div @click="triggerFileInput" class="group cursor-pointer active:scale-95 transition-all relative">
        <VcpAvatar 
          owner-type="user" 
          owner-id="user_avatar" 
          :fallback-name="settings.userName"
          size="w-16 h-16"
          rounded="rounded-full"
          dominant-color="var(--primary)"
        />
        <div class="absolute inset-0 rounded-full bg-black/40 opacity-0 group-hover:opacity-100 flex items-center justify-center z-20 transition-opacity">
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

  </SettingsCard>

  <!-- 头像裁剪器 -->
  <AvatarCropper v-if="isCropping" :img="cropImg" @cancel="cancelAvatarCrop" @confirm="onCropConfirm" />
</template>
