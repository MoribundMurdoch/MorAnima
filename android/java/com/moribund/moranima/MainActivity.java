package com.moribund.moranima;

import android.app.Activity;
import android.content.ContentValues;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;
import android.provider.MediaStore;
import android.util.Base64;
import android.webkit.JavascriptInterface;
import android.webkit.ValueCallback;
import android.webkit.WebChromeClient;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.widget.Toast;
import java.io.OutputStream;

/** WebView shell around the bundled MorAnima web studio (assets/index.html).
 *  Native side supplies the two things the page can't do alone in a WebView:
 *  file picking and saving exported files to Downloads. */
public class MainActivity extends Activity {
    private static final int PICK = 1;
    private ValueCallback<Uri[]> pendingChooser;

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        WebView web = new WebView(this);
        WebSettings s = web.getSettings();
        s.setJavaScriptEnabled(true);
        s.setDomStorageEnabled(true);
        s.setAllowFileAccess(true);
        s.setMediaPlaybackRequiresUserGesture(false);
        web.setWebChromeClient(new WebChromeClient() {
            @Override public boolean onShowFileChooser(
                    WebView v, ValueCallback<Uri[]> cb, FileChooserParams p) {
                if (pendingChooser != null) pendingChooser.onReceiveValue(null);
                pendingChooser = cb;
                Intent i = new Intent(Intent.ACTION_GET_CONTENT)
                        .addCategory(Intent.CATEGORY_OPENABLE);
                String[] types = p.getAcceptTypes();
                i.setType(types.length > 0 && !types[0].isEmpty() ? types[0] : "*/*");
                startActivityForResult(Intent.createChooser(i, "Choose file"), PICK);
                return true;
            }
        });
        web.addJavascriptInterface(new Saver(), "MorAnima");
        setContentView(web);
        web.loadUrl("file:///android_asset/index.html");
    }

    @Override protected void onActivityResult(int req, int res, Intent data) {
        if (req == PICK && pendingChooser != null) {
            Uri uri = (res == RESULT_OK && data != null) ? data.getData() : null;
            pendingChooser.onReceiveValue(uri != null ? new Uri[]{uri} : null);
            pendingChooser = null;
        }
    }

    private class Saver {
        @JavascriptInterface public void save(String name, String base64, String mime) {
            try {
                byte[] bytes = Base64.decode(base64, Base64.DEFAULT);
                ContentValues v = new ContentValues();
                v.put(MediaStore.Downloads.DISPLAY_NAME, name);
                v.put(MediaStore.Downloads.MIME_TYPE, mime);
                Uri uri = getContentResolver()
                        .insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, v);
                try (OutputStream os = getContentResolver().openOutputStream(uri)) {
                    os.write(bytes);
                }
                toast("Saved to Downloads: " + name);
            } catch (Exception e) {
                toast("Save failed: " + e.getMessage());
            }
        }
        private void toast(String msg) {
            runOnUiThread(() ->
                Toast.makeText(MainActivity.this, msg, Toast.LENGTH_LONG).show());
        }
    }
}
