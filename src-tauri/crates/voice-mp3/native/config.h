/* VCP Mobile's portable encoder-only configuration for LAME 4.0.
 * This is a local build input, not an upstream generated config.h.
 * Android/iOS ARM64 use LP64; the Windows test host uses LLP64. Rely on
 * stdint.h rather than copying configured long widths from either host.
 */
#ifndef VCP_VOICE_MP3_CONFIG_H
#define VCP_VOICE_MP3_CONFIG_H

#include <stdint.h>
#include <limits.h>
#include <float.h>

#define STDC_HEADERS 1
#define HAVE_STDINT_H 1
#define HAVE_INTTYPES_H 1
#define HAVE_ERRNO_H 1
#define HAVE_FCNTL_H 1
#define HAVE_LIMITS_H 1
#define HAVE_STRCHR 1
#define HAVE_MEMCPY 1
#define LAME_LIBRARY_BUILD 1
#define USE_FAST_LOG 1
#define PACKAGE "lame"
#define VERSION "4.0"

typedef float ieee754_float32_t;
typedef double ieee754_float64_t;

/* The Rust FFI uses c_short/c_int; fail at compile time on a mismatched ABI. */
typedef char vcp_pcm16_requires_16_bit_short[(sizeof(short) == 2 && CHAR_BIT == 8) ? 1 : -1];
typedef char vcp_lame_requires_32_bit_int[(sizeof(int) == 4) ? 1 : -1];
typedef char vcp_lame_requires_ieee_float[(sizeof(float) == 4 && FLT_RADIX == 2 && FLT_MANT_DIG == 24) ? 1 : -1];
typedef char vcp_lame_requires_64_bit_double[(sizeof(double) == 8) ? 1 : -1];

/* Leave HAVE_MPG123, DECODE_ON_THE_FLY, HAVE_NASM, HAVE_XMMINTRIN_H,
 * MMX_choose_table and TAKEHIRO_IEEE754_HACK undefined. All targets use
 * upstream portable C, including its portable quantization conversion.
 */
#endif
