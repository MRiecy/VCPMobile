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
   * 触发文件选择器并暂存附件 (Android 物理端使用原生选择拦截直传，其他端使用标准 HTML Input 完美支持)
   */
  const handleAttachment = async (mode: 'camera' | 'gallery' | 'file' = 'file') => {
    const isAndroid = navigator.userAgent.toLowerCase().includes("android");
    
    if (isAndroid) {
      console.log(`[AttachmentStore] Android environment detected. Intercepting via native picker. Mode: ${mode}`);
      const notificationStore = useNotificationStore();
      
      try {
        // 1. 调用物理端原生 File Picker (双轨事件监听 + 5分钟熔断)
        const stableId = `att_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`;
        
        const picked = await new Promise<any>((resolve, reject) => {
          let resolved = false;

          const handleStart = (e: any) => {
            if (resolved) return;
            const { name, size, mime } = e.detail;
            stagedAttachments.value.unshift({
              id: stableId,
              type: mime || "application/octet-stream",
              src: "",
              name: name || "文件",
              size: size || 0,
              progress: 0,
              status: "loading",
            });
          };

          const handleProgress = (e: any) => {
            if (resolved) return;
            const { progress } = e.detail;
            const idx = stagedAttachments.value.findIndex(a => a.id === stableId);
            if (idx !== -1) {
              stagedAttachments.value[idx].progress = progress;
            }
          };

          const handlePicked = (e: any) => {
            if (resolved) return;
            resolved = true;
            cleanup();
            console.log("[AttachmentStore] Native picker returned via EventBus:", e.detail);
            resolve(e.detail);
          };

          const cleanup = () => {
            window.removeEventListener('vcp-mobile-file-start', handleStart);
            window.removeEventListener('vcp-mobile-file-progress', handleProgress);
            window.removeEventListener('vcp-mobile-file-picked', handlePicked);
            clearTimeout(timer);
          };

          window.addEventListener('vcp-mobile-file-start', handleStart);
          window.addEventListener('vcp-mobile-file-progress', handleProgress);
          window.addEventListener('vcp-mobile-file-picked', handlePicked);

          const timer = setTimeout(() => {
            if (!resolved) {
              resolved = true;
              cleanup();
              reject(new Error("Native file picker timed out (5 mins) without reporting"));
            }
          }, 300000);

          invoke<any>("plugin:vcp-mobile|pick_file").then((res) => {
            if (!resolved) {
              resolved = true;
              cleanup();
              console.log("[AttachmentStore] Native picker returned via Invoke:", res);
              resolve(res);
            }
          }).catch((err) => {
             if (!resolved) {
               resolved = true;
               cleanup();
               reject(err);
             }
          });
        });
        
        if (!picked || !picked.path) {
          console.log("[AttachmentStore] Pick cancelled or returned empty path.");
          const existingIdx = stagedAttachments.value.findIndex(a => a.id === stableId);
          if (existingIdx !== -1) {
            stagedAttachments.value.splice(existingIdx, 1);
          }
          return;
        }

        // 兜底：如果卡片还没插入，补插一张
        const existingIdx = stagedAttachments.value.findIndex(a => a.id === stableId);
        if (existingIdx === -1) {
          stagedAttachments.value.unshift({
            id: stableId,
            type: picked.mime || "application/octet-stream",
            src: "",
            name: picked.name || "文件",
            size: picked.size || 0,
            progress: 100,
            status: "loading",
          });
        } else {
          stagedAttachments.value[existingIdx].progress = 100;
        }

        // 缩略图展示策略：若有 native thumbnail 物理路径则通过 convertFileSrc 转换，否则如果为图片，转换 path 自身
        let displaySrc = "";
        if (picked.thumbnailPath) {
          displaySrc = convertFileSrc(picked.thumbnailPath);
        } else if (picked.mime?.startsWith("image/")) {
          displaySrc = convertFileSrc(picked.path);
        }

        if (displaySrc) {
          const finalIdx = stagedAttachments.value.findIndex(a => a.id === stableId);
          if (finalIdx !== -1) {
            stagedAttachments.value[finalIdx].src = displaySrc;
          }
        }

        await nextTick();
        window.dispatchEvent(new Event("resize"));

        // 3. 后端零拷贝直传与注册 (会触发 rename 移动，缩略图生成，文本提取)
        const finalData = await invoke<any>("register_local_file", {
          localPath: picked.path,
          originalName: picked.name,
          mimeType: picked.mime || "application/octet-stream",
          thumbnailPath: picked.thumbnailPath || null,
        });

        if (finalData) {
          const index = stagedAttachments.value.findIndex((a) => a.id === stableId);
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
      } catch (err: any) {
        console.error("[AttachmentStore] Native file pick & registration failed:", err);
        notificationStore.addNotification({
          type: "warning",
          title: "选取附件失败",
          message: `❌ 异常捕获: ${err.message || String(err)}`,
          toastOnly: true,
        });
      }
      return;
    }

    // 非 Android 端的标准 HTML `<input>` 流程
    return new Promise<void>((resolve, reject) => {
      const input = document.createElement("input");
      input.type = "file";
      input.multiple = false;
      
      // 根据模式设置 accept 和 capture
      if (mode === 'camera') {
        input.accept = "image/*";
        input.setAttribute("capture", "environment");
      } else if (mode === 'gallery') {
        input.accept = "image/*";
      } else {
        input.accept = "*/*";
      }

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

  /**
   * 移除特定位置的暂存附件并触发后台 GC 物理清理
   */
  const removeStaged = (index: number) => {
    if (index >= 0 && index < stagedAttachments.value.length) {
      stagedAttachments.value.splice(index, 1);
      // 异步触发后台孤儿附件物理清理 (GC)
      invoke("cleanup_orphaned_attachments").catch((err) => {
        console.warn("[AttachmentStore] Background GC triggered on attachment removal failed:", err);
      });
    }
  };

  /**
   * 清空暂存附件并触发后台 GC 物理清理
   */
  const clearStaged = () => {
    stagedAttachments.value = [];
    // 异步触发后台孤儿附件物理清理 (GC)
    invoke("cleanup_orphaned_attachments").catch((err) => {
      console.warn("[AttachmentStore] Background GC triggered on staged clear failed:", err);
    });
  };

  return {
    stagedAttachments,
    handleAttachment,
    resolveMessageAssets,
    preProcessDocuments,
    removeStaged,
    clearStaged,
  };
});
