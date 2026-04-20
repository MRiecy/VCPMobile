import { AttachmentType } from '../types/AttachmentType';

/**
 * Ported from Rust file_manager.rs:get_refined_mime_type
 * Classifies attachments based on MIME type and file extension
 */
export function classifyAttachment(mimeType: string, fileName: string): AttachmentType {
  const ext = fileName
    .split('.')
    .pop()
    ?.toLowerCase() || '';

  // Force MP3 classification (matches Rust logic)
  if (ext === 'mp3') {
    return AttachmentType.AUDIO;
  }

  // If initial mime type is empty or generic, route by extension
  if (mimeType === '' || mimeType === 'application/octet-stream') {
    switch (ext) {
      case 'txt':
      case 'md':
      case 'json':
      case 'xml':
      case 'csv':
      case 'html':
      case 'css':
        return AttachmentType.TEXT;
      
      case 'pdf':
      case 'doc':
      case 'docx':
      case 'xls':
      case 'xlsx':
      case 'ppt':
      case 'pptx':
        return AttachmentType.DOCUMENT;
      
      case 'jpg':
      case 'jpeg':
      case 'png':
      case 'gif':
      case 'svg':
      case 'webp':
        return AttachmentType.IMAGE;
      
      case 'wav':
      case 'ogg':
      case 'flac':
      case 'aac':
      case 'aiff':
        return AttachmentType.AUDIO;
      
      case 'mp4':
      case 'webm':
        return AttachmentType.VIDEO;
      
      // All code/text files unified as text/plain (matches Rust logic)
      case 'js':
      case 'mjs':
      case 'bat':
      case 'sh':
      case 'py':
      case 'java':
      case 'c':
      case 'cpp':
      case 'h':
      case 'hpp':
      case 'cs':
      case 'go':
      case 'rb':
      case 'php':
      case 'swift':
      case 'kt':
      case 'kts':
      case 'ts':
      case 'tsx':
      case 'jsx':
      case 'vue':
      case 'yml':
      case 'yaml':
      case 'toml':
      case 'ini':
      case 'log':
      case 'sql':
      case 'jsonc':
      case 'rs':
      case 'dart':
      case 'lua':
      case 'r':
      case 'pl':
      case 'ex':
      case 'exs':
      case 'zig':
      case 'hs':
      case 'scala':
      case 'groovy':
      case 'd':
      case 'nim':
      case 'cr':
        return AttachmentType.CODE;
      
      default:
        break;
    }
  }

  // Classify by MIME type
  if (mimeType.startsWith('image/')) {
    return AttachmentType.IMAGE;
  } else if (mimeType.startsWith('video/')) {
    return AttachmentType.VIDEO;
  } else if (mimeType.startsWith('audio/')) {
    return AttachmentType.AUDIO;
  } else if (mimeType.startsWith('text/')) {
    return AttachmentType.TEXT;
  } else if (
    mimeType.includes('pdf') ||
    mimeType.includes('word') ||
    mimeType.includes('excel') ||
    mimeType.includes('powerpoint') ||
    mimeType.includes('msword') ||
    mimeType.includes('sheet') ||
    mimeType.includes('presentation')
  ) {
    return AttachmentType.DOCUMENT;
  } else if (
    mimeType.includes('javascript') ||
    mimeType.includes('ecmascript') ||
    mimeType.includes('json') ||
    mimeType.includes('xml')
  ) {
    return AttachmentType.CODE;
  } else if (
    mimeType.includes('application/') &&
    !mimeType.includes('octet-stream')
  ) {
    return AttachmentType.DOCUMENT;
  }

  return AttachmentType.OTHER;
}