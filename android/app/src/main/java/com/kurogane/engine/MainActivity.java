package com.kurogane.engine;

import android.Manifest;
import android.app.Activity;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Environment;
import android.provider.Settings;
import android.view.View;

import java.io.File;

/**
 * Kurogane launcher: an immersive full-screen host for {@link ControllerView}.
 *
 * Content discovery (like Ikemen GO): the app asks for storage access once,
 * then reads MUGEN data from a user-visible folder —
 *
 *   /storage/emulated/0/Kurogane    recommended (any file manager works)
 *   /storage/emulated/0/IkemenGO   existing Ikemen GO data auto-detected
 *   Android/data/com.kurogane.engine/files   USB/ADB fallback
 *
 * Storage access: Android 8/9 → classic storage permission dialog;
 * Android 10+ → the "All files access" toggle for this sideloaded APK.
 */
public class MainActivity extends Activity implements ControllerView.Host {

    static {
        System.loadLibrary("kurogane");
    }

    private static final int REQ_STORAGE = 26;

    private ControllerView view;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        hideSystemBars();
        view = new ControllerView(this, this);
        setContentView(view);
    }

    @Override
    protected void onResume() {
        super.onResume();
        // Also catches returning from the all-files-access Settings screen.
        rescan();
    }

    // ── immersive full-screen: status bar + nav bar hidden ──────────────────

    private void hideSystemBars() {
        getWindow().getDecorView().setSystemUiVisibility(
                View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                        | View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                        | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                        | View.SYSTEM_UI_FLAG_FULLSCREEN
                        | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                        | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN);
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) {
            hideSystemBars();
        }
    }

    // ── ControllerView.Host ─────────────────────────────────────────────────

    @Override
    public void onGrantStorage() {
        if (Build.VERSION.SDK_INT >= 30) {
            try {
                startActivity(new Intent(
                        Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
                        Uri.parse("package:" + getPackageName())));
            } catch (Exception e) {
                try {
                    startActivity(new Intent(
                            Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION));
                } catch (Exception ignored) {
                }
            }
        } else {
            requestPermissions(new String[]{
                    Manifest.permission.WRITE_EXTERNAL_STORAGE,
                    Manifest.permission.READ_EXTERNAL_STORAGE}, REQ_STORAGE);
        }
    }

    @Override
    public void onRescan() {
        rescan();
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions,
                                           int[] grantResults) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults);
        if (requestCode == REQ_STORAGE) {
            rescan();
        }
    }

    boolean hasStorageAccess() {
        if (Build.VERSION.SDK_INT >= 30) {
            return Environment.isExternalStorageManager();
        }
        return checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE)
                == PackageManager.PERMISSION_GRANTED;
    }

    // ── content scan ───────────────────────────────────────────────────────

    private void rescan() {
        if (view == null) {
            return;
        }
        view.setStatus("scanning…");
        final String roots = buildRoots();
        new Thread(() -> {
            final String report = nativeScan(roots);
            view.post(() -> view.applyScan(report, hasStorageAccess()));
        }).start();
    }

    private String buildRoots() {
        StringBuilder sb = new StringBuilder();
        File sd = Environment.getExternalStorageDirectory();
        appendRoot(sb, new File(sd, "Kurogane"));
        appendRoot(sb, new File(sd, "IkemenGO"));
        appendRoot(sb, new File(sd, "ikemen"));
        File ext = getExternalFilesDir(null);
        if (ext != null) {
            appendRoot(sb, ext);
        }
        appendRoot(sb, getFilesDir());
        return sb.toString();
    }

    private static void appendRoot(StringBuilder sb, File dir) {
        if (sb.length() > 0) {
            sb.append('\n');
        }
        sb.append(dir.getAbsolutePath());
    }

    private native String nativeScan(String roots);
}
