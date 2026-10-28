package com.kurogane.engine;

import android.Manifest;
import android.app.Activity;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.res.AssetManager;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Environment;
import android.provider.Settings;
import android.view.View;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;

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
 * The APK bundles the official Ikemen GO base data (screenpack, fonts, the
 * KFM characters and stages) in assets/basedata. On first run — after the
 * storage grant, when no content exists anywhere — it is extracted to
 * /storage/emulated/0/Kurogane automatically, so the folder the user would
 * otherwise create by hand appears ready to use. Users then add their own
 * chars/stages next to it and RESCAN.
 */
public class MainActivity extends Activity implements ControllerView.Host {

    static {
        System.loadLibrary("kurogane");
    }

    private static final int REQ_STORAGE = 26;

    /** Files that mark an already-installed base data set (skip re-extract). */
    private static final String[] BASE_MARKERS = {
            "data/fight.def", "data/system.base.def", "data/select.def"};

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

    // ── content scan + base-data install ────────────────────────────────────

    private void rescan() {
        if (view == null) {
            return;
        }
        view.setStatus("scanning…");
        new Thread(() -> {
            String report = nativeScan(buildRoots());
            if (!report.startsWith("OK") && hasStorageAccess()) {
                report = installBaseDataIfNeeded(report);
            }
            final String r = report;
            view.post(() -> view.applyScan(r, hasStorageAccess()));
        }).start();
    }

    /**
     * Extracts the bundled Ikemen GO base data to /sdcard/Kurogane when no
     * content was found anywhere. Idempotent: existing files are never
     * overwritten, so user edits and added chars survive reinstalls.
     */
    private String installBaseDataIfNeeded(String report) {
        File root = new File(Environment.getExternalStorageDirectory(), "Kurogane");
        try {
            boolean haveBase = false;
            for (String m : BASE_MARKERS) {
                if (new File(root, m).exists()) {
                    haveBase = true;
                    break;
                }
            }
            if (!haveBase) {
                int total = countAssetFiles("basedata");
                int[] copied = {0};
                postProgress(copied[0], total);
                copyAssetDir("basedata", root, total, copied);
                view.post(() -> view.setStatus("base data installed — scanning…"));
                return nativeScan(buildRoots());
            }
        } catch (IOException e) {
            report = "base data install failed: " + e + "\n" + report;
        }
        return report;
    }

    private void postProgress(int done, int total) {
        view.post(() -> view.setStatus(
                "installing base data… " + done + "/" + total + " files"));
    }

    private int countAssetFiles(String path) throws IOException {
        AssetManager am = getAssets();
        String[] kids = am.list(path);
        if (kids == null || kids.length == 0) {
            return 1;
        }
        int n = 0;
        for (String k : kids) {
            n += countAssetFiles(path + "/" + k);
        }
        return n;
    }

    private void copyAssetDir(String src, File dst, int total, int[] copied)
            throws IOException {
        AssetManager am = getAssets();
        String[] kids = am.list(src);
        if (kids == null || kids.length == 0) {
            // src names a file — dst is its full target path.
            if (!dst.exists()) {
                dst.getParentFile().mkdirs();
                try (InputStream in = am.open(src);
                     OutputStream out = new FileOutputStream(dst)) {
                    byte[] buf = new byte[65536];
                    for (int r; (r = in.read(buf)) > 0; ) {
                        out.write(buf, 0, r);
                    }
                }
            }
            copied[0]++;
            if (copied[0] % 25 == 0) {
                postProgress(copied[0], total);
            }
            return;
        }
        // src names a directory.
        dst.mkdirs();
        for (String k : kids) {
            copyAssetDir(src + "/" + k, new File(dst, k), total, copied);
        }
    }

    // ── content roots ───────────────────────────────────────────────────────

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
