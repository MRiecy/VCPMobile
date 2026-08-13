#ifndef VCP_MOBILE_TALLOC_REPLACE_H
#define VCP_MOBILE_TALLOC_REPLACE_H

#include <errno.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdarg.h>
#include <string.h>
#include <sys/auxv.h>
#include <sys/types.h>
#include <unistd.h>

#define TALLOC_BUILD_VERSION_MAJOR 2
#define TALLOC_BUILD_VERSION_MINOR 4
#define TALLOC_BUILD_VERSION_RELEASE 2

#define HAVE_SYS_AUXV_H 1
#define HAVE_INTPTR_T 1
#define HAVE_VA_COPY 1
#define HAVE_CONSTRUCTOR_ATTRIBUTE 1

#define VALGRIND_MAKE_MEM_UNDEFINED(pointer, length) do { (void)(pointer); (void)(length); } while (0)
#define VALGRIND_MAKE_MEM_DEFINED(pointer, length) do { (void)(pointer); (void)(length); } while (0)
#define VALGRIND_MAKE_MEM_NOACCESS(pointer, length) do { (void)(pointer); (void)(length); } while (0)

#define ZERO_STRUCT(value) memset((char *)&(value), 0, sizeof(value))
#define discard_const(pointer) ((void *)((uintptr_t)(pointer)))
#define MIN(left, right) ((left) < (right) ? (left) : (right))
#define MAX(left, right) ((left) > (right) ? (left) : (right))

#endif
