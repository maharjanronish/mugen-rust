package com.kurogane.engine;

import android.content.Context;
import android.opengl.GLSurfaceView;

import javax.microedition.khronos.egl.EGLConfig;
import javax.microedition.khronos.opengles.GL10;

/**
 * The game surface: a GLES 3.0 GLSurfaceView whose render thread owns the
 * whole native renderer (krg-gfx). The surface sits at the bottom of the
 * view stack; the ControllerView overlay composites on top of it.
 *
 * Rendering is vsync-driven and continuous (fighting game); in setup mode
 * the native side just clears to black.
 */
public final class GameSurfaceView extends GLSurfaceView {

    public GameSurfaceView(Context context) {
        super(context);
        // GLES 3.0 — the project's floor (krg-gfx: batching, FBO, texture LRU).
        setEGLContextClientVersion(3);
        // Survive pause/resume without destroying the GL context when the
        // platform allows it (no re-uploads of the whole texture LRU).
        setPreserveEGLContextOnPause(true);
        setRenderer(new Renderer() {
            @Override
            public void onSurfaceCreated(GL10 gl, EGLConfig config) {
                nativeGlInit();
            }

            @Override
            public void onSurfaceChanged(GL10 gl, int width, int height) {
                nativeGlResize(width, height);
            }

            @Override
            public void onDrawFrame(GL10 gl) {
                nativeGlDraw();
            }
        });
        setRenderMode(RENDERMODE_CONTINUOUSLY);
    }

    private static native void nativeGlInit();

    private static native void nativeGlResize(int w, int h);

    private static native void nativeGlDraw();
}
