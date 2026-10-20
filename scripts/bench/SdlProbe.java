import org.lwjgl.sdl.SDLError;
import org.lwjgl.sdl.SDLInit;
import org.lwjgl.sdl.SDLVideo;

import static org.lwjgl.sdl.SDLInit.SDL_INIT_VIDEO;
import static org.lwjgl.sdl.SDLInit.SDL_Init;
import static org.lwjgl.sdl.SDLError.SDL_GetError;
import static org.lwjgl.sdl.SDLVideo.SDL_CreateWindow;
import static org.lwjgl.sdl.SDLVideo.SDL_DestroyWindow;
import static org.lwjgl.sdl.SDLVideo.SDL_GL_SetAttribute;
import static org.lwjgl.sdl.SDLVideo.SDL_WINDOW_HIDDEN;
import static org.lwjgl.sdl.SDLVideo.SDL_WINDOW_OPENGL;

/** Reproduce the renderpearl GlDevice SDL3/GLX window creation on this X server. */
public final class SdlProbe {
    public static void main(String[] args) {
        if (!SDL_Init(SDL_INIT_VIDEO)) {
            System.out.println("SDL_Init failed: " + SDL_GetError());
            return;
        }
        int depth = args.length > 0 ? Integer.parseInt(args[0]) : 24;
        boolean minimal = args.length > 1 && args[1].equals("min");
        setAttr(SDLVideo.SDL_GL_RED_SIZE, 8);
        setAttr(SDLVideo.SDL_GL_GREEN_SIZE, 8);
        setAttr(SDLVideo.SDL_GL_BLUE_SIZE, 8);
        setAttr(SDLVideo.SDL_GL_ALPHA_SIZE, 8);
        setAttr(SDLVideo.SDL_GL_DOUBLEBUFFER, 1);
        if (!minimal) {
            setAttr(SDLVideo.SDL_GL_DEPTH_SIZE, depth);
            setAttr(SDLVideo.SDL_GL_STENCIL_SIZE, 8);
        }
        long w = SDL_CreateWindow("probe", 320, 480, SDL_WINDOW_OPENGL | SDL_WINDOW_HIDDEN);
        System.out.println("depth=" + depth + (minimal ? " min" : "") + ": window=" + w
                + (w == 0 ? " err=" + SDL_GetError() : ""));
        if (w != 0) SDL_DestroyWindow(w);
        SDLInit.SDL_Quit();
    }

    private static void setAttr(int attr, int value) {
        if (!SDL_GL_SetAttribute(attr, value)) {
            System.out.println("SetAttribute " + attr + " failed: " + SDL_GetError());
        }
    }
}
