import { defineStore } from "pinia";
import { ref, nextTick } from "vue";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { useDocumentProcessor } from "../composables/useDocumentProcessor";
import { useNotificationStore } from "./notification";
import type { Attachment } from "../types/chat";

/**
 * 前端辅助：异步读取图片原始分辨率（不依赖后端）
 * 用于上传前拦截超限图片（>8K×8K）
 */
const checkImageDimensions = (file: File): Promise<{ width: number; height: number }> => {
  return new Promise((resolve, reject) => {
    const img = new Image();
    const url = URL.createObjectURL(file);
    img.onload = () => {
      URL.revokeObjectURL(url);
      resolve({ width: img.naturalWidth, height: img.naturalHeight });
    };
    img.onerror = () => {
      URL.revokeObjectURL(url);
      reject(new Error("无法读取图片尺寸"));
    };
    img.src = url;
  });
};

export const useAttachmentStore = defineStore("attachment", () => {
  // 暂存的附件列表，准备随下一条消息发送
  const stagedAttachments = ref<Attachment[]>([]);

  /**
   * 处理消息中的本地资源路径 (仅附件)，使用 Tauri 原生 asset:// 协议绕过 WebView 限制
   */
  const resolveMessageAssets = (msg: any) => {
    // 处理附件 (仅处理图片类型)
    if (msg.attachments && msg.attachments.length > 0) {
      msg.attachments.forEach((att: Attachment) => {
        // Rust 后端返回的路径现在主要在 internalPath，如果不在，回退到 src
        const sourcePath = att.internalPath || att.src;
        if (
          att.type.startsWith("image/") &&
          sourcePath &&
          !sourcePath.startsWith("http") &&
          !sourcePath.startsWith("data:")
        ) {
          try {
            att.resolvedSrc = convertFileSrc(sourcePath);
          } catch (err) {
            console.warn(
              `[AttachmentStore] Failed to convert attachment image path ${att.name}:`,
              err,
            );
          }
        }
      });
    }
  };

  /**
   * 触发文件选择器并暂存附件 (使用标准 HTML Input 完美解决 Android content:// 协议名和类型丢失问题)
   */
  const handleAttachment = async () => {
    return new Promise<void>((resolve, reject) => {
      const input = document.createElement("input");
      input.type = "file";
      input.multiple = false;
      // 允许所有类型
      input.accept = "*/*";

      input.onchange = async (e: Event) => {
        try {
          const target = e.target as HTMLInputElement;
          if (!target.files || target.files.length === 0) {
            resolve();
            return;
          }

          const file = target.files[0];
          console.log(
            `[AttachmentStore] Selected file via HTML input: ${file.name}, type: ${file.type}, size: ${file.size}`,
          );

          const ext = file.name.split('.').pop()?.toLowerCase() || '';
          const isGif = ext === 'gif' || file.type === 'image/gif';
          const isImage = file.type.startsWith('image/');
          const notificationStore = useNotificationStore();

          // 1. 大小拦截：非 GIF 图片 > 10MB 直接拒绝
          if (isImage && !isGif && file.size > 10 * 1024 * 1024) {
            notificationStore.addNotification({
              type: "warning",
              title: "图片过大",
              message: "图片过大（>10MB），请压缩后重试。",
              toastOnly: true,
            });
            resolve();
            return;
          }

          // 2. 分辨率拦截：非 GIF 图片 > 8Kx8K 直接拒绝
          if (isImage && !isGif) {
            try {
              const dims = await checkImageDimensions(file);
              if (dims.width > 8192 || dims.height > 8192) {
                notificationStore.addNotification({
                  type: "warning",
                  title: "分辨率过高",
                  message: "图片分辨率过高（>8K），请压缩后重试。",
                  toastOnly: true,
                });
                resolve();
                return;
              }
            } catch (e) {
              console.warn("[AttachmentStore] Failed to check image dimensions:", e);
              // 尺寸检测失败不阻断上传，继续
            }
          }

          // 3. 生成稳定 ID 并使用 unshift 插入首位 (实现"最新附件最先看到")
          const stableId = `att_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`;
          const blobUrl = URL.createObjectURL(file);

          stagedAttachments.value.unshift({
            id: stableId,
            type: file.type || "application/octet-stream",
            src: blobUrl,
            name: file.name,
            size: file.size,
            status: "loading",
          });

          await nextTick();
          window.dispatchEvent(new Event("resize"));

          try {
            let finalData: any = null;

            // --- 分流策略：小文件 ( < 2MB ) 走 IPC，大文件走高速 TCP 链路 ---
            if (file.size < 2 * 1024 * 1024) {
              console.log(
                `[AttachmentStore] Small file detected (<2MB), using store_file IPC for ${file.name}`,
              );
              // 将 File 转换为 Uint8Array (Tauri v2 支持直接传递二进制)
              const arrayBuffer = await file.arrayBuffer();
              const bytes = new Uint8Array(arrayBuffer);

              finalData = await invoke<any>("store_file", {
                originalName: file.name,
                fileBytes: bytes, 
                mimeType: file.type || "application/octet-stream",
              });
            } else {
              console.log(
                `[AttachmentStore] Large file detected, opening High-Speed Link for ${file.name} (${file.size} bytes)`,
              );

              // 1. 准备链路 (Rust 开启临时本地 TCP 接收器)
              const endpoint = await invoke<any>("prepare_vcp_upload", {
                metadata: {
                  name: file.name,
                  mime: file.type || "application/octet-stream",
                  size: file.size,
                },
              });

              // 2. 内核级搬运 (利用流式上传)
              const xhr = new XMLHttpRequest();
              const uploadPromise = new Promise((res, rej) => {
                xhr.open("POST", endpoint.url, true);
                xhr.setRequestHeader(
                  "Content-Type",
                  "application/octet-stream",
                );
                xhr.setRequestHeader("X-Upload-Token", endpoint.token);

                let lastUpdate = 0;
                xhr.upload.onprogress = (event) => {
                  if (event.lengthComputable) {
                    const now = Date.now();
                    // 限制刷新频率为 ~30fps (每 33ms 刷新一次)，避免高频重绘导致卡顿
                    if (now - lastUpdate < 33) return;
                    lastUpdate = now;

                    const progress = Math.round(
                      (event.loaded / event.total) * 100,
                    );
                    const attIndex = stagedAttachments.value.findIndex(
                      (a) => a.id === stableId,
                    );
                    if (attIndex !== -1) {
                      stagedAttachments.value[attIndex].progress = progress;
                    }
                  }
                };

                xhr.onload = () => {
                  if (xhr.status >= 200 && xhr.status < 300) {
                    res(JSON.parse(xhr.responseText));
                  } else {
                    rej(new Error(`Upload failed with status ${xhr.status}`));
                  }
                };

                xhr.onerror = () => rej(new Error("XHR Network Error"));
                xhr.send(file);
              });

              finalData = await uploadPromise;
            }

            if (finalData) {
              const index = stagedAttachments.value.findIndex(
                (a) => a.id === stableId,
              );
              if (index !== -1) {
                stagedAttachments.value[index] = {
                  ...stagedAttachments.value[index],
                  type: finalData.type,
                  src: finalData.internalPath,
                  name: finalData.name,
                  size: finalData.size,
                  hash: finalData.hash,
                  status: "done",
                };
              }
            }
            resolve();
          } catch (err) {
            console.error("[AttachmentStore] High-speed upload failed:", err);
            const index = stagedAttachments.value.findIndex(
              (a) => a.id === stableId,
            );
            if (index !== -1) stagedAttachments.value.splice(index, 1);
            reject(err);
          } finally {
            URL.revokeObjectURL(blobUrl);
          }
          resolve();
        } catch (err) {
          console.error(
            "[AttachmentStore] Failed to pick or store attachment:",
            err,
          );
          reject(err);
        }
      };

      input.oncancel = () => {
        resolve();
      };

      input.click();
    });
  };

  /**
   * 消息发送前的文档预处理 (JIT)
   */
  const preProcessDocuments = async (customList?: Attachment[]) => {
    const targetList = customList || stagedAttachments.value;
    if (targetList.length === 0) return;
    
    const docProcessor = useDocumentProcessor();
    for (const att of targetList) {
      const ext = att.name.split(".").pop()?.toLowerCase();
      // Only process documents and PDFs as requested
      if (["txt", "md", "csv", "json", "docx", "pdf"].includes(ext || "")) {
        try {
          const result = await docProcessor.processAttachment(att);
          if (result) {
            if (result.extractedText)
              att.extractedText = result.extractedText;
            if (result.imageFrames) att.imageFrames = result.imageFrames;
          }
        } catch (e) {
          console.error(
            `[AttachmentStore] JIT document processing failed for ${att.name}:`,
            e,
          );
        }
      }
    }
  };

  const clearStaged = () => {
    stagedAttachments.value = [];
  };

  return {
    stagedAttachments,
    handleAttachment,
    resolveMessageAssets,
    preProcessDocuments,
    clearStaged,
  };
});
