LOCAL_PATH := $(call my-dir)

include $(CLEAR_VARS)
LOCAL_MODULE := vcp_pty
LOCAL_SRC_FILES := vcp_pty.cpp
LOCAL_CPPFLAGS := -std=c++17 -Wall -Wextra -Werror
LOCAL_LDLIBS := -llog
include $(BUILD_SHARED_LIBRARY)
