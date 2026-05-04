import * as pdfjsLib from "pdfjs-dist";
import mammoth from "mammoth";
import { Attachment } from "../types/chat";
import { convertFileSrc } from "@tauri-apps/api/core";

// Configure PDF.js worker
pdfjsLib.GlobalWorkerOptions.workerSrc = `https://cdnjs.cloudflare.com/ajax/libs/pdf.js/${pdfjsLib.version}/pdf.worker.min.mjs`;

export interface DocumentProcessResult {
  extractedText?: string;
  imageFrames?: string[]; // Array of base64 strings (without data URI prefix)
}

export function useDocumentProcessor() {
  /**
   * Processes a local file attachment and extracts text or image frames.
   */
  const processAttachment = async (
    att: Attachment
  ): Promise<DocumentProcessResult | null> => {
    try {
      // For mobile, the file is already selected and its path is in internalPath or src
      const sourcePath = att.internalPath || att.src;
      if (!sourcePath) return null;

      // Ensure we can fetch it in the webview
      let fetchUrl = sourcePath;
      if (!sourcePath.startsWith("http") && !sourcePath.startsWith("blob:") && !sourcePath.startsWith("data:")) {
        fetchUrl = convertFileSrc(sourcePath);
      }

      const response = await fetch(fetchUrl);
      const arrayBuffer = await response.arrayBuffer();

      const ext = att.name.split(".").pop()?.toLowerCase();

      // 1. Plain Text Processing
      if (["txt", "md", "csv", "json"].includes(ext || "")) {
        const text = new TextDecoder("utf-8").decode(arrayBuffer);
        return { extractedText: text };
      }

      // 2. Docx Processing (using Mammoth)
      if (ext === "docx") {
        const result = await mammoth.extractRawText({ arrayBuffer });
        return { extractedText: result.value };
      }

      // 3. PDF Processing (using PDF.js)
      if (ext === "pdf" || att.type === "application/pdf") {
        return await processPdf(arrayBuffer, att.name);
      }

      return null;
    } catch (e) {
      console.error(`[DocumentProcessor] Failed to process ${att.name}:`, e);
      return null;
    }
  };

  /**
   * Handles PDF text extraction and fallback to image rasterization.
   */
  const processPdf = async (
    arrayBuffer: ArrayBuffer,
    filename: string
  ): Promise<DocumentProcessResult> => {
    const loadingTask = pdfjsLib.getDocument({ data: arrayBuffer });
    const pdf = await loadingTask.promise;

    let fullText = "";

    // Try to extract text first
    for (let i = 1; i <= pdf.numPages; i++) {
      const page = await pdf.getPage(i);
      const textContent = await page.getTextContent();
      const pageText = textContent.items
        .map((item: any) => item.str)
        .join(" ");
      fullText += pageText + "\n";
    }

    const trimmedText = fullText.trim();
    const letterCount = (trimmedText.match(/[a-zA-Z]/g) || []).length;

    // Heuristic: If it has enough text, it's a text-based PDF
    if (trimmedText.length > 20 && letterCount > 10) {
      console.log(`[DocumentProcessor] Successfully extracted text from PDF ${filename}`);
      return { extractedText: trimmedText };
    }

    // Otherwise, treat as Scanned PDF and convert pages to JPEG images
    console.log(`[DocumentProcessor] PDF ${filename} appears scanned. Rasterizing pages...`);
    const imageFrames: string[] = [];
    const MAX_DIMENSION = 800; // Match desktop standard

    for (let i = 1; i <= pdf.numPages; i++) {
      const page = await pdf.getPage(i);
      const viewport = page.getViewport({ scale: 1.0 });

      // Calculate scale to fit within MAX_DIMENSION
      const scale = Math.min(MAX_DIMENSION / viewport.width, MAX_DIMENSION / viewport.height, 2.0); // Max scale 2.0 to prevent memory blowup
      const scaledViewport = page.getViewport({ scale });

      const canvas = document.createElement("canvas");
      const context = canvas.getContext("2d");
      if (!context) continue;

      canvas.height = scaledViewport.height;
      canvas.width = scaledViewport.width;

      // Render page to canvas
      await page.render({
        canvasContext: context,
        viewport: scaledViewport,
        canvas: canvas,
      }).promise;

      // Convert to Base64 JPEG (Quality 0.7 matches Desktop)
      const dataUrl = canvas.toDataURL("image/jpeg", 0.7);

      // Desktop expects raw base64 string without the prefix
      const base64Data = dataUrl.split(",")[1];
      if (base64Data) {
        imageFrames.push(base64Data);
      }
    }

    return {
      extractedText: `[VChat Auto-summary: This is a scanned PDF named "${filename}". The content is displayed as images.]`,
      imageFrames,
    };
  };

  return {
    processAttachment,
  };
}