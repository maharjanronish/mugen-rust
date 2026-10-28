package com.kurogane.engine;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.graphics.Typeface;
import android.view.Choreographer;
import android.view.MotionEvent;
import android.view.View;

/**
 * The Kurogane minimal fighting controller.
 *
 * Everything that matters lives in Rust (krg-input): zone geometry, hit
 * testing, chord synthesis (X+Y→Z, A+B→C, A+X→D, B+Y→W, all-four→M). This
 * view forwards touches and renders what Rust computes — one source of truth.
 *
 * Layout: 8-way pad (left), four attack buttons X Y / A B (right), a small
 * START (top-right). Heavy attacks and Ikemen's extra buttons are chords,
 * so every MUGEN move stays reachable with clean minimal controls.
 */
public final class ControllerView extends View {

    /** Must match krg_input::touch zone kinds. */
    private static final int ZONE_PAD = 0;
    private static final int ZONE_BTN = 1;
    private static final int ZONE_START = 2;

    private static final String[] BTN_NAME = {"A", "B", "C", "X", "Y", "Z"};

    private static final int BG = 0xFF0E0E12;
    private static final int ACCENT = 0xFFD2232A;

    private final Paint zoneFill = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint zoneLine = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint label = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint hud = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint knob = new Paint(Paint.ANTI_ALIAS_FLAG);

    private String status = "scanning content…";
    private String held = "";

    private final Choreographer choreographer = Choreographer.getInstance();
    private final Choreographer.FrameCallback frameCb = new Choreographer.FrameCallback() {
        @Override
        public void doFrame(long timeNanos) {
            nativeInputTick();
            invalidate();
            choreographer.postFrameCallback(this);
        }
    };

    public ControllerView(Context context) {
        super(context);
        setKeepScreenOn(true);

        zoneFill.setStyle(Paint.Style.FILL);
        zoneLine.setStyle(Paint.Style.STROKE);
        zoneLine.setStrokeWidth(3f);
        zoneLine.setColor(0xAAFFFFFF);

        label.setColor(Color.WHITE);
        label.setTypeface(Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD));
        label.setTextAlign(Paint.Align.CENTER);

        knob.setStyle(Paint.Style.FILL);
        knob.setColor(Color.WHITE);

        hud.setColor(0xFFE6E6E6);
        hud.setTypeface(Typeface.MONOSPACE);
        hud.setTextAlign(Paint.Align.LEFT);
    }

    public void setStatus(String s) {
        this.status = s;
    }

    @Override
    protected void onAttachedToWindow() {
        super.onAttachedToWindow();
        choreographer.postFrameCallback(frameCb);
    }

    @Override
    protected void onDetachedFromWindow() {
        super.onDetachedFromWindow();
        choreographer.removeFrameCallback(frameCb);
    }

    @Override
    public boolean onTouchEvent(MotionEvent e) {
        final int action = e.getActionMasked();
        final int idx = e.getActionIndex();
        switch (action) {
            case MotionEvent.ACTION_DOWN:
            case MotionEvent.ACTION_POINTER_DOWN:
                send(e, idx, 0);
                break;
            case MotionEvent.ACTION_MOVE:
                for (int i = 0; i < e.getPointerCount(); i++) {
                    send(e, i, 1);
                }
                break;
            case MotionEvent.ACTION_POINTER_UP:
            case MotionEvent.ACTION_UP:
                send(e, idx, 2);
                break;
            case MotionEvent.ACTION_CANCEL:
                for (int i = 0; i < e.getPointerCount(); i++) {
                    send(e, i, 2);
                }
                break;
        }
        return true;
    }

    private void send(MotionEvent e, int i, int phase) {
        nativeInputTouch(e.getPointerId(i), phase,
                e.getX(i), e.getY(i), getWidth(), getHeight());
    }

    @Override
    protected void onDraw(Canvas c) {
        c.drawColor(BG);
        float w = getWidth();
        float h = getHeight();

        // Fresh zones + held state from Rust every frame (cheap).
        float[] zones = nativeInputZones(w, h);
        if (zones != null) {
            for (int i = 0; i + 7 < zones.length; i += 8) {
                int type = (int) zones[i];
                int param = (int) zones[i + 1];
                float cx = zones[i + 2];
                float cy = zones[i + 3];
                float r = zones[i + 4];
                boolean heldZone = zones[i + 5] != 0;
                float ex = zones[i + 6];
                float ey = zones[i + 7];

                zoneFill.setColor(heldZone ? 0xCCD2232A : 0x2EFFFFFF);
                c.drawCircle(cx, cy, r, zoneFill);
                c.drawCircle(cx, cy, r, zoneLine);

                if (type == ZONE_PAD) {
                    // Floating knob showing the current direction.
                    float kx = cx + ex * r * 0.45f;
                    float ky = cy + ey * r * 0.45f;
                    c.drawCircle(kx, ky, r * 0.32f, knob);
                    if (heldZone) {
                        c.drawCircle(kx, ky, r * 0.32f, zoneFill);
                    }
                } else {
                    String name = type == ZONE_START ? "≡" : BTN_NAME[param];
                    label.setTextSize(r * 0.9f);
                    float ty = cy - (label.descent() + label.ascent()) / 2f;
                    c.drawText(name, cx, ty, label);
                }
            }
        }

        // HUD: held buttons + chord cheatsheet + content status.
        float pad = 24f;
        hud.setTextSize(30f);
        String d = nativeInputDescribe();
        if (d.isEmpty()) {
            d = "—";
        }
        c.drawText("HELD: " + d, pad, pad + 20f, hud);
        hud.setTextSize(22f);
        hud.setColor(0xFF9A9AA5);
        c.drawText("X+Y=Z   A+B=C   A+X=D   B+Y=W   ALL=M", pad, pad + 56f, hud);

        // Content report (wrapped crudely at ~46 chars, up to 8 lines).
        hud.setTextSize(20f);
        hud.setColor(0xFF8A8A93);
        float y = pad + 96f;
        String[] lines = status.split("\n");
        int n = 0;
        for (String line : lines) {
            c.drawText(line, pad, y, hud);
            y += 26f;
            if (++n >= 9) {
                break;
            }
        }
    }

    // ── native (krg-android) ────────────────────────────────────────────────

    private native void nativeInputTouch(long id, int phase, float x, float y, float w, float h);

    private native int nativeInputTick();

    private native int nativeInputHeld();

    private native float[] nativeInputZones(float w, float h);

    private native String nativeInputDescribe();
}
