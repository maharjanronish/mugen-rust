package com.kurogane.engine;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.graphics.RectF;
import android.graphics.Typeface;
import android.os.Environment;
import android.view.Choreographer;
import android.view.MotionEvent;
import android.view.View;

import java.io.File;

/**
 * The Kurogane view.
 *
 * With content present: the minimal fighting controller. Everything that
 * matters lives in Rust (krg-input): zone geometry, hit testing, chord
 * synthesis (X+Y→Z, A+B→C, A+X→D, B+Y→W, all-four→M). This view forwards
 * touches and renders what Rust computes — one source of truth.
 *
 * Without content: a setup panel explaining where MUGEN data goes
 * (/storage/emulated/0/Kurogane), with GRANT ACCESS / RESCAN buttons.
 */
public final class ControllerView extends View {

    /** Launcher callbacks (permission flow, rescan). */
    public interface Host {
        void onGrantStorage();

        void onRescan();
    }

    /** Must match krg_input::touch zone kinds. */
    private static final int ZONE_PAD = 0;
    private static final int ZONE_BTN = 1;
    private static final int ZONE_START = 2;

    private static final String[] BTN_NAME = {"A", "B", "C", "X", "Y", "Z"};

    private static final int BG = 0xFF0E0E12;
    private static final int ACCENT = 0xFFD2232A;

    private final Host host;

    private final Paint zoneFill = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint zoneLine = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint label = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint hud = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint knob = new Paint(Paint.ANTI_ALIAS_FLAG);

    // Setup-panel paints.
    private final Paint title = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint head = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint small = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint mono = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint tiny = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint btnText = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint panelFill = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint accentFill = new Paint();

    private final RectF btnGrant = new RectF();
    private final RectF btnRescan = new RectF();
    private final RectF pathBox = new RectF();

    private String status = "scanning…";
    private boolean setupMode = false;
    private boolean storageGranted = false;
    private String setupPath;

    private final Choreographer choreographer = Choreographer.getInstance();
    private final Choreographer.FrameCallback frameCb = new Choreographer.FrameCallback() {
        @Override
        public void doFrame(long timeNanos) {
            nativeInputTick();
            invalidate();
            choreographer.postFrameCallback(this);
        }
    };

    public ControllerView(Context context, Host host) {
        super(context);
        this.host = host;
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

        title.setColor(Color.WHITE);
        title.setTypeface(Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD));
        title.setTextAlign(Paint.Align.CENTER);

        head.setColor(Color.WHITE);
        head.setTypeface(Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD));
        head.setTextAlign(Paint.Align.CENTER);

        small.setColor(0xFFC9C9D2);
        small.setTextAlign(Paint.Align.CENTER);

        mono.setColor(Color.WHITE);
        mono.setTypeface(Typeface.create(Typeface.MONOSPACE, Typeface.BOLD));
        mono.setTextAlign(Paint.Align.CENTER);

        tiny.setColor(0xFF8A8A93);
        tiny.setTextAlign(Paint.Align.CENTER);

        btnText.setColor(Color.WHITE);
        btnText.setTypeface(Typeface.create(Typeface.SANS_SERIF, Typeface.BOLD));
        btnText.setTextAlign(Paint.Align.CENTER);

        panelFill.setColor(0x12FFFFFF);

        setupPath = new File(Environment.getExternalStorageDirectory(),
                "Kurogane").getAbsolutePath();
    }

    public void setStatus(String s) {
        this.status = s;
    }

    /** Report comes from nativeScan; first line "OK:" means content found. */
    public void applyScan(String report, boolean granted) {
        storageGranted = granted;
        status = report;
        setupMode = report == null || !report.startsWith("OK");
        invalidate();
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
        if (setupMode) {
            if (e.getActionMasked() == MotionEvent.ACTION_DOWN) {
                if (btnRescan.contains(e.getX(), e.getY())) {
                    host.onRescan();
                } else if (!storageGranted && btnGrant.contains(e.getX(), e.getY())) {
                    host.onGrantStorage();
                }
            }
            return true;
        }
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
        if (setupMode) {
            drawSetup(c, getWidth(), getHeight());
            return;
        }
        float w = getWidth();
        float h = getHeight();
        float pad = 24f;

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

        // HUD: held buttons + chord cheatsheet + content report.
        hud.setColor(0xFFE6E6E6);
        hud.setTextSize(30f);
        String d = nativeInputDescribe();
        if (d.isEmpty()) {
            d = "—";
        }
        c.drawText("HELD: " + d, pad, pad + 20f, hud);
        hud.setTextSize(22f);
        hud.setColor(0xFF9A9AA5);
        c.drawText("X+Y=Z   A+B=C   A+X=D   B+Y=W   ALL=M", pad, pad + 56f, hud);

        hud.setTextSize(19f);
        hud.setColor(0xFF8A8A93);
        drawWrapped(c, status, pad, pad + 96f, hud, w - pad * 2f, 16);
    }

    // ── setup panel ─────────────────────────────────────────────────────────

    private void drawSetup(Canvas c, float w, float h) {
        float cx = w / 2f;

        title.setTextSize(Math.min(w / 10f, 88f));
        c.drawText("KUROGANE", cx, h * 0.14f, title);
        accentFill.setColor(ACCENT);
        float uw = Math.min(title.measureText("KUROGANE") / 2f, 170f);
        c.drawRect(cx - uw, h * 0.14f + 16f, cx + uw, h * 0.14f + 24f, accentFill);

        head.setTextSize(32f);
        c.drawText("no MUGEN content found — setup required", cx, h * 0.235f, head);

        small.setTextSize(22f);
        c.drawText("put your MUGEN data in:", cx, h * 0.315f, small);

        float bw = Math.min(w * 0.8f, 780f);
        pathBox.set(cx - bw / 2f, h * 0.335f, cx + bw / 2f, h * 0.335f + 84f);
        c.drawRoundRect(pathBox, 16f, 16f, panelFill);
        zoneLine.setColor(0x66FFFFFF);
        zoneLine.setStrokeWidth(2f);
        c.drawRoundRect(pathBox, 16f, 16f, zoneLine);
        mono.setTextSize(30f);
        float ty = pathBox.centerY() - (mono.descent() + mono.ascent()) / 2f;
        c.drawText(setupPath, cx, ty, mono);

        small.setTextSize(20f);
        c.drawText("copy  data/  +  font/  from an Ikemen GO release (or MUGEN 1.x)",
                cx, pathBox.bottom + 48f, small);
        c.drawText("then add  chars/<name>/<name>.def  and  stages/<name>.def",
                cx, pathBox.bottom + 80f, small);

        float btnW = Math.min(430f, w * 0.36f);
        float btnH = 92f;
        float gap = 28f;
        float by = h * 0.62f;
        float bx = (w - btnW * 2f - gap) / 2f;
        btnGrant.set(bx, by, bx + btnW, by + btnH);
        btnRescan.set(bx + btnW + gap, by, bx + btnW * 2f + gap, by + btnH);
        drawButton(c, btnGrant, "GRANT ACCESS", !storageGranted);
        drawButton(c, btnRescan, "RESCAN", true);

        small.setTextSize(22f);
        small.setColor(storageGranted ? 0xFF9FE8A0 : 0xFFF0B8A0);
        c.drawText(storageGranted
                ? "storage access: granted — add data, then RESCAN"
                : "storage access: not granted — tap GRANT ACCESS first",
                cx, by + btnH + 48f, small);
        small.setColor(0xFFC9C9D2);

        tiny.setTextSize(16f);
        c.drawText("fallback: USB/ADB  Android/data/com.kurogane.engine/files/data",
                cx, h - 26f, tiny);

        // Restore shared paint state mutated above.
        zoneLine.setStrokeWidth(3f);
        zoneLine.setColor(0xAAFFFFFF);
        panelFill.setColor(0x12FFFFFF);
    }

    private void drawButton(Canvas c, RectF r, String text, boolean enabled) {
        panelFill.setColor(enabled ? 0x26FFFFFF : 0x10FFFFFF);
        c.drawRoundRect(r, 20f, 20f, panelFill);
        zoneLine.setColor(enabled ? 0xAAFFFFFF : 0x40FFFFFF);
        c.drawRoundRect(r, 20f, 20f, zoneLine);
        btnText.setColor(enabled ? Color.WHITE : 0x66FFFFFF);
        btnText.setTextSize(30f);
        float ty = r.centerY() - (btnText.descent() + btnText.ascent()) / 2f;
        c.drawText(text, r.centerX(), ty, btnText);
    }

    /** Greedy word-agnostic wrap; returns the y after the last drawn line. */
    private void drawWrapped(Canvas c, String text, float x, float y,
                             Paint p, float maxW, int maxLines) {
        int lines = 0;
        for (String para : text.split("\n")) {
            if (para.isEmpty()) {
                y += p.getTextSize() * 1.35f;
                if (++lines >= maxLines) {
                    return;
                }
                continue;
            }
            String line = para;
            while (!line.isEmpty()) {
                int n = p.breakText(line, true, maxW, null);
                if (n <= 0) {
                    n = 1;
                }
                c.drawText(line, 0, n, x, y, p);
                y += p.getTextSize() * 1.35f;
                line = line.substring(Math.min(n, line.length())).trim();
                if (++lines >= maxLines) {
                    return;
                }
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
