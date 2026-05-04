import { ref, computed } from 'vue';
import { AttachmentRegistry } from '../AttachmentRegistry';
import { AttachmentType } from '../types/AttachmentType';
import { classifyAttachment } from '../utils/AttachmentClassifier';
import type { Attachment } from '../../../../core/types/chat';

/**
 * Composable function for attachment preview functionality
 * Provides reactive state and methods for handling attachment previews
 */
export function useAttachmentPreview() {
  const isLoading = ref(false);
  const error = ref<string | null>(null);
  const previewCache = new Map<string, string>();

  /**
   * Get the component type for an attachment
   */
  const getAttachmentType = (attachment: Attachment): AttachmentType => {
    return classifyAttachment(attachment.type, attachment.name);
  };

  /**
   * Check if an attachment can be previewed
   */
  const canPreview = computed(() => {
    return (attachment: Attachment): boolean => {
      const type = getAttachmentType(attachment);
      // Images, videos, and text files can always be previewed
      if ([AttachmentType.IMAGE, AttachmentType.VIDEO, AttachmentType.TEXT].includes(type)) {
        return true;
      }
      // Audio files can be previewed if they have a source
      if (type === AttachmentType.AUDIO && attachment.src) {
        return true;
      }
      // Documents can be previewed if they have extracted text
      if (type === AttachmentType.DOCUMENT && attachment.extractedText) {
        return true;
      }
      // Code files can be previewed if they have extracted text
      if (type === AttachmentType.CODE && attachment.extractedText) {
        return true;
      }
      return false;
    };
  });

  /**
   * Get the preview component for an attachment
   */
  const getPreviewComponent = computed(() => {
    return (attachment: Attachment) => {
      const type = getAttachmentType(attachment);
      return AttachmentRegistry.getComponent(type);
    };
  });

  /**
   * Format file size in human readable format
   */
  const formatFileSize = (bytes: number): string => {
    if (bytes === 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
  };

  /**
   * Get file extension from attachment name
   */
  const getFileExtension = (attachment: Attachment): string => {
    const ext = attachment.name.split('.').pop()?.toLowerCase() || '';
    return ext;
  };

  /**
   * Get preview text for text-based attachments
   */
  const getPreviewText = computed(() => {
    return (attachment: Attachment, maxLength: number = 100): string => {
      if (!attachment.extractedText) return '';
      
      const text = attachment.extractedText;
      if (text.length <= maxLength) return text;
      
      // Try to break at word boundaries
      const truncated = text.substring(0, maxLength);
      const lastSpace = truncated.lastIndexOf(' ');
      
      return lastSpace > 0 ? truncated.substring(0, lastSpace) + '...' : truncated + '...';
    };
  });

  /**
   * Check if an attachment has a thumbnail
   */
  const hasThumbnail = computed(() => {
    return (attachment: Attachment): boolean => {
      return !!attachment.thumbnailPath;
    };
  });

  /**
   * Clear preview cache
   */
  const clearCache = (): void => {
    previewCache.clear();
    console.log('[useAttachmentPreview] Cache cleared');
  };

  /**
   * Get attachment statistics
   */
  const getStats = computed(() => {
    return (attachments: Attachment[]) => {
      const stats = {
        total: attachments.length,
        byType: {} as Record<AttachmentType, number>,
        canPreview: 0,
        hasText: 0,
        hasThumbnails: 0,
      };

      attachments.forEach(attachment => {
        const type = getAttachmentType(attachment);
        stats.byType[type] = (stats.byType[type] || 0) + 1;
        
        if (canPreview.value(attachment)) {
          stats.canPreview++;
        }
        
        if (attachment.extractedText) {
          stats.hasText++;
        }
        
        if (hasThumbnail.value(attachment)) {
          stats.hasThumbnails++;
        }
      });

      return stats;
    };
  });

  return {
    isLoading,
    error,
    previewCache,
    getAttachmentType,
    canPreview,
    getPreviewComponent,
    formatFileSize,
    getFileExtension,
    getPreviewText,
    hasThumbnail,
    clearCache,
    getStats,
  };
}
