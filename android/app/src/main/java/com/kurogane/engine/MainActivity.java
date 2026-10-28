package com.kurogane.engine;

import android.app.Activity;
import android.os.Bundle;

/**
 * Kurogane launcher. Hosts {@link ControllerView} — the minimal touch
 * controller demo (zone geometry and chord logic live in Rust) — and runs a
 * background content scan whose report is shown in the view's HUD.
 *
 * Content goes in: Android/data/com.kurogane.engine/files/data/
 */
public class MainActivity extends Activity {

    static {
        System.loadLibrary("kurogane");
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        final ControllerView view = new ControllerView(this);
        setContentView(view);

        // Background content scan; report lands in the HUD when ready.
        new Thread(() -> {
            String dir = null;
            java.io.File ext = getExternalFilesDir(null);
            if (ext == null) {
                ext = getFilesDir();
            }
            if (ext != null) {
                dir = ext.getAbsolutePath();
            }
            final String report = nativeHello(dir != null ? dir : "");
            view.post(() -> view.setStatus(report));
        }).start();
    }

    private native String nativeHello(String filesDir);
}
