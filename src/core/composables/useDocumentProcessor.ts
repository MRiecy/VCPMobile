import { Attachment } from "../types/chat";
import { convertFileSrc } from "@tauri-apps/api/core";

export interface DocumentProcessResult {
  extractedText?: string;
  imageFrames?: string[]; // Array of base64 strings (without data URI prefix)
}

export function useDocumentProcessor() {
  /**
   * Processes a local file attachment and extracts text or image frames.
   * Note: Word/PDF/Excel/PPT text extraction has been fully offloaded to the Rust backend for robust security and memory efficiency.
   */
  const processAttachment = async (
    att: Attachment
  ): Promise<DocumentProcessResult | null> => {
    try {
      // For mobile, the file is already selected and its path is in internalPath or src
      const sourcePath = att.internalPath || att.src;
      if (!sourcePath) return null;

      const ext = att.name.split(".").pop()?.toLowerCase() || '';

      // Only handle basic text types on the frontend if needed immediately
      if (["txt", "md", "csv", "json"].includes(ext)) {
        let fetchUrl = sourcePath;
        if (!sourcePath.startsWith("http") && !sourcePath.startsWith("blob:") && !sourcePath.startsWith("data:")) {
          fetchUrl = convertFileSrc(sourcePath.replace("file://", ""));
        }

        const response = await fetch(fetchUrl);
        const arrayBuffer = await response.arrayBuffer();
        const text = new TextDecoder("utf-8").decode(arrayBuffer);
        return { extractedText: text };
      }

      // PDF, Docx, Xlsx, Pptx extraction is handled securely in the Rust backend
      return null;
    } catch (e) {
      console.error(`[DocumentProcessor] Failed to process ${att.name}:`, e);
      return null;
    }
  };

  return {
    processAttachment,
  };
}