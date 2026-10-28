package com.kurogane.engine;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.graphics.PorterDuff;
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
 * With content present: the fighting controller. Everything that matters
 * lives in Rust (krg-input): zone geometry, hit testing, chord synthesis
 * (any two punches add the missing one, likewise kicks; A+X→D, B+Y→W,
 * all-four→M). This view forwards touches and renders what Rust
 * computes — one source of truth.
 *
 * The layout is user-editable: tap the small EDIT button (beside START).
 * In edit mode you drag controls to move them, pinch
 * (or the −/+ steppers) to resize, DEFAULTS restores the factory layout,
 * and SAVE/CANCEL commits or discards. The layout persists in
 * files/controller.ini and is restored on startup.
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

    /** EDIT toggle placement (normalized; sits beside START, top-right). */
    private static final float EDIT_CX = 0.845f;
    private static final float EDIT_CY = 0.085f;
    private static final float EDIT_R = 0.030f;

    /** Toolbar button ids. */
    private static final int TB_MINUS = 0;
    private static final int TB_PLUS = 1;
    private static final int TB_DEFAULTS = 2;
    private static final int TB_CANCEL = 3;
    private static final int TB_SAVE = 4;

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
    private final Paint panelFill = new Paint();
    private final Paint accentFill = new Paint();

    private final RectF btnGrant = new RectF();
    private final RectF btnRescan = new RectF();
    private final RectF pathBox = new RectF();

    /** Edit toolbar rects, indexed by TB_*. */
    private final RectF[] toolbar = {
            new RectF(), new RectF(), new RectF(), new RectF(), new RectF()};

    private String status = "scanning…";
    private boolean setupMode = false;
    private boolean storageGranted = false;
    private String setupPath;

    // ── layout editor state (all UI-side; geometry stays in Rust) ──────────
    private boolean editMode = false;
    /** Selected control for resize/steppers: (kind, param) of the zone. */
    private int selKind = -1;
    private int selParam = -1;
    /** Pointer currently dragging the selection (id from MotionEvent). */
    private int dragPointer = -1;
    /** Finger→center offset at drag start so controls don't jump to the finger. */
    private float grabDx = 0f;
    private float grabDy = 0f;
    /** Two fingers down → pinch-resize the selection. */
    private boolean pinching = false;
    private float pinchSpan = 0f;

    /** Persisted controller layout (files/controller.ini). */
    private final String layoutPath;

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
        layoutPath = new File(context.getFilesDir(), "controller.ini").getAbsolutePath();
        // Saved layout (dragged buttons etc.) — falls back to defaults if absent.
        nativeInputLayoutLoad(layoutPath);
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
        if (editMode) {
            return editTouch(e);
        }
        return playTouch(e);
    }

    // ── play mode: forward to Rust + EDIT affordances ───────────────────────

    private boolean playTouch(MotionEvent e) {
        final int action = e.getActionMasked();
        final int idx = e.getActionIndex();
        switch (action) {
            case MotionEvent.ACTION_DOWN:
            case MotionEvent.ACTION_POINTER_DOWN: {
                float x = e.getX(idx);
                float y = e.getY(idx);
                if (hitEditButton(x, y)) {
                    enterEditMode();
                    return true;
                }
                send(e, idx, 0);
                break;
            }
            case MotionEvent.ACTION_MOVE: {
                for (int i = 0; i < e.getPointerCount(); i++) {
                    send(e, i, 1);
                }
                break;
            }
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

    /** Open the editor: snapshot the layout, clear held buttons, show toolbar. */
    private void enterEditMode() {
        nativeInputEditBegin(); // Rust snapshots the layout + clears held buttons
        editMode = true;
        selKind = -1;
        selParam = -1;
        dragPointer = -1;
        pinching = false;
        pinchSpan = 0f;
        invalidate();
    }

    private void exitEditMode(boolean save) {
        if (save) {
            nativeInputLayoutSave(layoutPath);
        } else {
            nativeInputEditRevert();
        }
        nativeInputReleaseAll();
        editMode = false;
        selKind = -1;
        selParam = -1;
        dragPointer = -1;
        pinching = false;
        invalidate();
    }

    // ── edit mode: drag / pinch / toolbar ────────────────────────────────────

    private boolean editTouch(MotionEvent e) {
        final int action = e.getActionMasked();
        final int idx = e.getActionIndex();
        switch (action) {
            case MotionEvent.ACTION_DOWN:
            case MotionEvent.ACTION_POINTER_DOWN: {
                float x = e.getX(idx);
                float y = e.getY(idx);
                int tb = hitToolbar(x, y);
                if (tb >= 0) {
                    fireToolbar(tb);
                    return true;
                }
                int[] z = pickZone(x, y);
                if (z != null) {
                    selKind = z[0];
                    selParam = z[1];
                    float[] c = zoneCenter(selKind, selParam);
                    grabDx = c[0] - x;
                    grabDy = c[1] - y;
                    dragPointer = e.getPointerId(idx);
                } else {
                    dragPointer = -1; // empty space: pinch anchor only
                }
                if (e.getPointerCount() >= 2) {
                    pinching = true;
                    pinchSpan = span(e);
                }
                invalidate();
                break;
            }
            case MotionEvent.ACTION_MOVE: {
                if (pinching && e.getPointerCount() >= 2) {
                    float s = span(e);
                    if (selKind >= 0 && pinchSpan > 1f && s > 1f) {
                        float f = s / pinchSpan;
                        if (Math.abs(f - 1f) > 0.002f) {
                            nativeInputEditScale(selKind, selParam, f);
                        }
                    }
                    pinchSpan = s;
                } else {
                    pinching = false;
                    if (selKind >= 0 && dragPointer >= 0) {
                        int i = e.findPointerIndex(dragPointer);
                        if (i >= 0) {
                            nativeInputEditMove(selKind, selParam,
                                    e.getX(i) + grabDx, e.getY(i) + grabDy,
                                    getWidth(), getHeight());
                        }
                    }
                }
                invalidate();
                break;
            }
            case MotionEvent.ACTION_POINTER_UP:
            case MotionEvent.ACTION_UP:
                if (e.getPointerId(idx) == dragPointer) {
                    dragPointer = -1; // selection stays for pinch/steppers
                }
                if (e.getPointerCount() <= 2) {
                    pinching = false;
                    pinchSpan = 0f;
                }
                invalidate();
                break;
            case MotionEvent.ACTION_CANCEL:
                dragPointer = -1;
                pinching = false;
                invalidate();
                break;
        }
        return true;
    }

    private void fireToolbar(int tb) {
        switch (tb) {
            case TB_MINUS:
                if (selKind >= 0) {
                    nativeInputEditScale(selKind, selParam, 1f / 1.15f);
                }
                break;
            case TB_PLUS:
                if (selKind >= 0) {
                    nativeInputEditScale(selKind, selParam, 1.15f);
                }
                break;
            case TB_DEFAULTS:
                nativeInputResetLayout();
                break;
            case TB_CANCEL:
                exitEditMode(false);
                return;
            case TB_SAVE:
                exitEditMode(true);
                return;
        }
        invalidate();
    }

    /** Distance between the first two pointers (pinch span). */
    private static float span(MotionEvent e) {
        if (e.getPointerCount() < 2) {
            return 0f;
        }
        return (float) Math.hypot(e.getX(0) - e.getX(1), e.getY(0) - e.getY(1));
    }

    // ── zone queries (pixel space, from Rust geometry) ──────────────────────

    /** Zone under (x, y) as {kind, param}, nearest center wins; null = miss. */
    private int[] pickZone(float x, float y) {
        float[] zs = nativeInputZones(getWidth(), getHeight());
        if (zs == null) {
            return null;
        }
        int[] best = null;
        float bestD = Float.MAX_VALUE;
        for (int i = 0; i + 7 < zs.length; i += 8) {
            float d = (float) Math.hypot(x - zs[i + 2], y - zs[i + 3]);
            if (d < zs[i + 4] * 1.2f && d < bestD) {
                bestD = d;
                best = new int[]{(int) zs[i], (int) zs[i + 1]};
            }
        }
        return best;
    }

    /** Center (px) of the zone identified by (kind, param); null if absent. */
    private float[] zoneCenter(int kind, int param) {
        float[] zs = nativeInputZones(getWidth(), getHeight());
        if (zs == null) {
            return null;
        }
        for (int i = 0; i + 7 < zs.length; i += 8) {
            if ((int) zs[i] == kind && (int) zs[i + 1] == param) {
                return new float[]{zs[i + 2], zs[i + 3]};
            }
        }
        return null;
    }

    // ── rendering ───────────────────────────────────────────────────────────

    @Override
    protected void onDraw(Canvas c) {
        if (setupMode) {
            c.drawColor(BG);
            drawSetup(c, getWidth(), getHeight());
            return;
        }
        // Play mode: transparent so the GLES game surface shows through
        // (this view composites above it in the same window).
        c.drawColor(Color.TRANSPARENT, PorterDuff.Mode.CLEAR);
        if (editMode) {
            // Readable backdrop over the game while editing.
            c.drawColor(0x8C0E0E12);
        }
        float w = getWidth();
        float h = getHeight();
        float m = Math.min(w, h);
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
                boolean selected = editMode && type == selKind && param == selParam;

                zoneFill.setColor(selected ? 0x55D2232A : (heldZone ? 0xCCD2232A : 0x2EFFFFFF));
                c.drawCircle(cx, cy, r, zoneFill);
                c.drawCircle(cx, cy, r, zoneLine);

                if (selected) {
                    zoneLine.setStrokeWidth(6f);
                    zoneLine.setColor(ACCENT);
                    c.drawCircle(cx, cy, r + 10f, zoneLine);
                    zoneLine.setStrokeWidth(3f);
                    zoneLine.setColor(0xAAFFFFFF);
                }

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

                if (selected) {
                    small.setTextSize(Math.max(r * 0.5f, 22f));
                    c.drawText(zoneLabel(type, param), cx, cy - r - 26f, small);
                }
            }
        }

        if (editMode) {
            drawEditor(c, w, h, pad);
            return;
        }

        // EDIT toggle (UI chrome — never forwarded to the game controller).
        float ex = EDIT_CX * w;
        float ey = EDIT_CY * h;
        float er = EDIT_R * m;
        zoneFill.setColor(0x2EFFFFFF);
        c.drawCircle(ex, ey, er, zoneFill);
        c.drawCircle(ex, ey, er, zoneLine);
        label.setTextSize(er * 0.55f);
        float ety = ey - (label.descent() + label.ascent()) / 2f;
        c.drawText("EDIT", ex, ety, label);

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
        c.drawText("tap EDIT to customize · hold 2 buttons for extra signals",
                pad, pad + 86f, hud);

        hud.setTextSize(19f);
        hud.setColor(0xFF8A8A93);
        drawWrapped(c, status, pad, pad + 124f, hud, w - pad * 2f, 14);
    }

    private static String zoneLabel(int type, int param) {
        if (type == ZONE_PAD) {
            return "PAD";
        }
        if (type == ZONE_START) {
            return "START";
        }
        return "BUTTON " + BTN_NAME[param];
    }

    /** Editor chrome: instructions + the −/+/DEFAULTS/CANCEL/SAVE toolbar. */
    private void drawEditor(Canvas c, float w, float h, float pad) {
        float cx = w / 2f;

        head.setTextSize(34f);
        c.drawText("EDIT CONTROLS", cx, pad + 26f, head);
        small.setTextSize(20f);
        c.drawText("drag to move · pinch or − + to resize · tap a control first",
                cx, pad + 58f, small);

        String[] texts = {"−", "+", "DEFAULTS", "CANCEL", "SAVE"};
        float bh = 84f;
        float gap = 18f;
        btnText.setTextSize(30f);
        float[] bw = new float[texts.length];
        float total = 0f;
        for (int i = 0; i < texts.length; i++) {
            bw[i] = Math.max(btnText.measureText(texts[i]) + 68f, 96f);
            total += bw[i];
        }
        total += gap * (texts.length - 1);
        float maxTotal = w - 48f;
        if (total > maxTotal) { // narrow screens: shrink to fit
            float k = maxTotal / total;
            for (int i = 0; i < texts.length; i++) {
                bw[i] *= k;
            }
            total = maxTotal;
        }
        float x = cx - total / 2f;
        float y = pad + 78f;
        for (int i = 0; i < texts.length; i++) {
            toolbar[i].set(x, y, x + bw[i], y + bh);
            boolean primary = i == TB_SAVE;
            boolean needsSel = i == TB_MINUS || i == TB_PLUS;
            boolean enabled = !needsSel || selKind >= 0;
            panelFill.setColor(primary ? ACCENT : (enabled ? 0x26FFFFFF : 0x10FFFFFF));
            c.drawRoundRect(toolbar[i], 18f, 18f, panelFill);
            zoneLine.setStrokeWidth(primary ? 0f : 2f);
            if (!primary) {
                zoneLine.setColor(enabled ? 0x66FFFFFF : 0x40FFFFFF);
                c.drawRoundRect(toolbar[i], 18f, 18f, zoneLine);
            }
            btnText.setColor(enabled ? Color.WHITE : 0x66FFFFFF);
            btnText.setTextSize(i <= TB_PLUS ? 40f : 30f);
            float ty = toolbar[i].centerY() - (btnText.descent() + btnText.ascent()) / 2f;
            c.drawText(texts[i], toolbar[i].centerX(), ty, btnText);
            x += bw[i] + gap;
        }
        zoneLine.setStrokeWidth(3f);
        zoneLine.setColor(0xAAFFFFFF);
        panelFill.setColor(0x12FFFFFF);
        btnText.setColor(Color.WHITE);

        tiny.setTextSize(16f);
        c.drawText("changes apply instantly · SAVE persists, CANCEL undoes everything",
                cx, y + bh + 34f, tiny);
    }

    /** Toolbar button under (x, y), or -1. */
    private int hitToolbar(float x, float y) {
        for (int i = 0; i < toolbar.length; i++) {
            if (toolbar[i].contains(x, y)) {
                return i;
            }
        }
        return -1;
    }

    private boolean hitEditButton(float x, float y) {
        float m = Math.min(getWidth(), getHeight());
        return Math.hypot(x - EDIT_CX * getWidth(), y - EDIT_CY * getHeight())
                < EDIT_R * m * 1.6f;
    }

    private void send(MotionEvent e, int i, int phase) {
        nativeInputTouch(e.getPointerId(i), phase,
                e.getX(i), e.getY(i), getWidth(), getHeight());
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

    private native void nativeInputReleaseAll();

    private native void nativeInputEditBegin();

    private native void nativeInputEditMove(int kind, int param, float x, float y,
                                            float w, float h);

    private native void nativeInputEditScale(int kind, int param, float factor);

    private native void nativeInputEditRevert();

    private native void nativeInputResetLayout();

    private native boolean nativeInputLayoutSave(String path);

    private native boolean nativeInputLayoutLoad(String path);
}
