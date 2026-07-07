/**
 * 统一的应用程序异常类，支持错误编码、用户提示和原始错误跟踪。
 */
export class AppError extends Error {
  public code: string;
  public userMessage: string;
  public originalError?: any;

  constructor(code: string, userMessage: string, originalError?: any) {
    super(`[${code}] ${userMessage}`);
    this.name = 'AppError';
    this.code = code;
    this.userMessage = userMessage;
    this.originalError = originalError;

    // 恢复原型链以保证 instanceof 正常工作
    Object.setPrototypeOf(this, AppError.prototype);
  }

  /**
   * 静态方法：从任意未知错误中安全转换或包装为 AppError
   */
  static from(error: any, defaultCode = 'UNKNOWN_ERROR', defaultUserMessage = '系统开小差了，请稍后再试'): AppError {
    if (error instanceof AppError) {
      return error;
    }

    if (error instanceof Error) {
      return new AppError(defaultCode, error.message, error);
    }

    if (typeof error === 'string') {
      return new AppError(defaultCode, error);
    }

    return new AppError(defaultCode, defaultUserMessage, error);
  }
}
