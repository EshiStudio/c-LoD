/* libclodgl.so — an SDL "GL library" shim for the c-LoD test bench.
 *
 * Mesa's dril GLX on this Xvfb exposes no sRGB-capable visuals, and renderpearl
 * requests SDL_GL_FRAMEBUFFER_SRGB_CAPABLE=1, so glXChooseFBConfig matches
 * nothing and the client dies with "Couldn't find matching GLX visual".
 * SDL dlopens this shim instead (SDL_VIDEO_GL_DRIVER / SDL_OPENGL_LIBRARY);
 * every symbol forwards to the real libGL.so.1, except the two FBConfig/visual
 * choosers, which strip the GLX_FRAMEBUFFER_SRGB_CAPABLE pair first.
 * Software-rendered sRGB-less configs are perfectly fine for a test bench.
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <unistd.h>

typedef void Display;
typedef unsigned long GLXDrawable;
typedef void GLXFBConfig;
typedef void GLXContext;
typedef void XVisualInfo;

static void *real_handle(void) {
    static void *h;
    if (!h) h = dlopen("/lib/x86_64-linux-gnu/libGL.so.1", RTLD_NOW | RTLD_GLOBAL);
    if (!h) fprintf(stderr, "[libclodgl] dlopen libGL failed: %s\n", dlerror());
    return h;
}
__attribute__((constructor)) static void clodgl_ctor(void) {
    fprintf(stderr, "[libclodgl] shim loaded (pid %d)\n", getpid());
}
static void *rsym(const char *n) {
    void *h = real_handle();
    void *s = h ? dlsym(h, n) : NULL;
    if (!s) fprintf(stderr, "[libclodgl] missing real symbol: %s\n", n);
    return s;
}

/* GLX_FRAMEBUFFER_SRGB_CAPABLE_ARB=0x20B2, _EXT=0x20B1 */
static int *strip_srgb(const int *attrib_list) {
    if (!attrib_list) return NULL;
    int n = 0;
    while (attrib_list[n] != 0) n += 2;
    int *out = (int *)malloc((n + 2) * sizeof(int));
    int k = 0;
    for (int i = 0; i < n; i += 2) {
        if (attrib_list[i] == 0x20B1 || attrib_list[i] == 0x20B2) continue;
        out[k++] = attrib_list[i];
        out[k++] = attrib_list[i + 1];
    }
    out[k] = 0;
    return out;
}

/* ---------- the two filtered entry points ---------- */

GLXFBConfig *glXChooseFBConfig(Display *dpy, int screen, const int *attrib_list, int *nelements) {
    static GLXFBConfig *(*p)(Display *, int, const int *, int *);
    if (!p) p = (GLXFBConfig *(*)(Display *, int, const int *, int *))rsym("glXChooseFBConfig");
    if (!p) return NULL;
    int n = 0;
    if (attrib_list) while (attrib_list[n] != 0) n += 2;
    fprintf(stderr, "[libclodgl] glXChooseFBConfig called (%d attribs)\n", n / 2);
    int *clean = strip_srgb(attrib_list);
    GLXFBConfig *r = p(dpy, screen, clean, nelements);
    fprintf(stderr, "[libclodgl] glXChooseFBConfig -> %s\n", r ? "configs" : "NULL");
    if (clean) free(clean);
    return r;
}

XVisualInfo *glXChooseVisual(Display *dpy, int screen, int *attrib_list) {
    static XVisualInfo *(*p)(Display *, int, int *);
    if (!p) p = (XVisualInfo *(*)(Display *, int, int *))rsym("glXChooseVisual");
    if (!p) return NULL;
    fprintf(stderr, "[libclodgl] glXChooseVisual called\n");
    int *clean = strip_srgb(attrib_list);
    XVisualInfo *r = p(dpy, screen, clean);
    fprintf(stderr, "[libclodgl] glXChooseVisual -> %s\n", r ? "visual" : "NULL");
    if (clean) free(clean);
    return r;
}

/* ---------- everything else forwards verbatim ---------- */

#define B  /* bool */ int
#define D  Display *
#define C  void *   /* GLXContext is itself a pointer; pass as void* (incomplete) */
#define F  GLXFBConfig *
#define FC void *   /* GLXFBConfig is itself a pointer; pass as void* (incomplete) */
#define V  XVisualInfo *
#define DR GLXDrawable

#define FWD(ret, name, proto, args)                                    \
    ret name proto {                                                   \
        static ret (*_fnptr) proto;                                    \
        if (!_fnptr) _fnptr = (ret (*)proto)rsym(#name);               \
        return _fnptr ? _fnptr args : (ret)0;                          \
    }

FWD(int, glXQueryExtension,        (D d, int *e, int *v),           (d, e, v))
FWD(int, glXQueryVersion,          (D d, int *maj, int *min),       (d, maj, min))
FWD(const char *, glXQueryExtensionsString, (D d, int s),           (d, s))
FWD(const char *, glXQueryServerString,     (D d, int s, int n),    (d, s, n))
FWD(const char *, glXGetClientString,       (D d, int n),           (d, n))
FWD(V,   glXGetVisualFromFBConfig, (D d, FC cfg),                   (d, cfg))
FWD(int, glXGetFBConfigAttrib,     (D d, FC cfg, int a, int *val),  (d, cfg, a, val))
FWD(F,   glXGetFBConfigs,          (D d, int s, int *n),            (d, s, n))
FWD(C,   glXCreateContext,         (D d, V vis, C share, B direct), (d, vis, share, direct))
FWD(C,   glXCreateNewContext,      (D d, F cfg, int rt, C sh, B dr),(d, cfg, rt, sh, dr))
FWD(C,   glXCreateContextAttribsARB,(D d, F cfg, C sh, B dr, const int *at), (d, cfg, sh, dr, at))
FWD(B,   glXMakeCurrent,           (D d, DR draw, C c),             (d, draw, c))
FWD(B,   glXMakeContextCurrent,    (D d, DR draw, DR read, C c),    (d, draw, read, c))
FWD(B,   glXIsDirect,              (D d, C c),                      (d, c))
FWD(void, glXDestroyContext,       (D d, C c),                      (d, c))
FWD(void, glXSwapBuffers,          (D d, DR draw),                  (d, draw))
FWD(int, glXSwapIntervalSGI,       (int i),                         (i))
FWD(int, glXSwapIntervalMESA,      (unsigned i),                    (i))
FWD(int, glXSwapIntervalEXT,       (D d, DR draw, int i),           (d, draw, i))
FWD(C,   glXGetCurrentContext,     (void),                          ())
FWD(D,   glXGetCurrentDisplay,     (void),                          ())
FWD(DR,  glXGetCurrentDrawable,    (void),                          ())
FWD(DR,  glXGetCurrentReadDrawable,(void),                          ())
FWD(void, glXWaitGL,               (void),                          ())
FWD(void, glXWaitX,                (void),                          ())
FWD(void, glXCopyContext,          (D d, C s, C dst, unsigned long m), (d, s, dst, m))
FWD(void, glXUseXFont,             (void *f, int a, int b, int c),  (f, a, b, c))
FWD(DR,  glXCreatePbuffer,         (D d, F cfg, const int *at),     (d, cfg, at))
FWD(void, glXDestroyPbuffer,       (D d, DR p),                     (d, p))
FWD(DR,  glXCreatePixmap,          (D d, F cfg, void *pix, const int *at), (d, cfg, pix, at))
FWD(void, glXDestroyPixmap,        (D d, DR p),                     (d, p))
FWD(DR,  glXCreateWindow,          (D d, F cfg, unsigned long win, const int *at), (d, cfg, win, at))
FWD(void, glXDestroyWindow,        (D d, DR w),                     (d, w))
FWD(int, glXQueryContext,          (D d, C c, int a, int *v),       (d, c, a, v))
FWD(void, glXSelectEvent,          (D d, DR draw, unsigned long m), (d, draw, m))
FWD(void, glXGetSelectedEvent,     (D d, DR draw, unsigned long *m),(d, draw, m))

typedef void (*__GLXextFuncPtr)(void);

static __GLXextFuncPtr clodgl_lookup(const unsigned char *n) {
    if (n) {
        if (!strcmp((const char *)n, "glXChooseFBConfig")) return (__GLXextFuncPtr)&glXChooseFBConfig;
        if (!strcmp((const char *)n, "glXChooseVisual"))  return (__GLXextFuncPtr)&glXChooseVisual;
    }
    return (__GLXextFuncPtr)rsym(n ? (const char *)n : "");
}

__GLXextFuncPtr glXGetProcAddress(const unsigned char *n)    { return clodgl_lookup(n); }
__GLXextFuncPtr glXGetProcAddressARB(const unsigned char *n) { return clodgl_lookup(n); }
