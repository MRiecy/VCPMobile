package com.vcp.mobile.webview

import android.app.Activity
import android.content.ComponentCallbacks2
import android.content.res.Configuration
import android.graphics.Bitmap
import android.graphics.Rect
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.PixelCopy
import android.view.View
import android.view.ViewGroup
import android.view.ViewTreeObserver
import android.webkit.WebView
import android.widget.FrameLayout
import org.json.JSONObject
import java.util.UUID

/** Owns one native, process-local last frame. It never changes WebView visibility or its client. */
internal class NativeWebViewResume(private val activity: Activity) : ComponentCallbacks2 {
    private data class Capture(
        val bitmap: Bitmap,
        val attachment: Long,
        val cycle: Long,
        var accepted: Boolean = true,
    )

    private data class Ready(val documentId: String, val token: String)
    private data class DrawAttempt(val ready: Ready, val attachment: Long, val serial: Long)

    private val main = Handler(Looper.getMainLooper())
    private var webView: WebView? = null
    private var attachment = 0L
    private var cycle = 0L
    private var serial = 0L
    private var paused = false
    private var stopped = false
    private var resumed = false
    private var returning = false
    private var memorySuppressed = false
    private var destroyed = false
    private var hasDrawn = false
    private var capture: Capture? = null
    private var snapshot: Bitmap? = null
    private var snapshotDisplayed = false
    private var cover: ResumeCoverView? = null
    private var coveredRect: Rect? = null
    private var previousAccessibility: Int? = null
    private var documentId: String? = null
    private var token: String? = null
    private var pendingReady: Ready? = null
    private var drawAttempt: DrawAttempt? = null
    private var drawObserver: ViewTreeObserver? = null
    private var frameObserver: ViewTreeObserver? = null
    private var frameCallback: Runnable? = null
    private var afterDraw: Runnable? = null
    private var nextFrame: Runnable? = null
    private var timeout: Runnable? = null

    private val layoutListener = View.OnLayoutChangeListener { _, _, _, _, _, _, _, _, _ ->
        updateGeometry()
    }
    private val attachListener = object : View.OnAttachStateChangeListener {
        override fun onViewAttachedToWindow(view: View) {
            if (view === webView && !destroyed) installDrawObserver()
        }

        override fun onViewDetachedFromWindow(view: View) {
            if (view !== webView) return
            attachment += 1
            cancelDrawWork()
            cancelTimeout()
            capture?.accepted = false
            removeCover()
            releaseSnapshot()
            removeDrawObserver()
            token = null
            pendingReady = null
            documentId = null
            hasDrawn = false
        }
    }
    private val drawListener = ViewTreeObserver.OnDrawListener {
        val target = webView
        if (target != null && target.isAttachedToWindow && target.isShown) {
            hasDrawn = true
            drawAttempt?.let { attempt ->
                if (isCurrent(attempt)) {
                    drawAttempt = null
                    waitForDrawSubmission(target, attempt)
                }
            }
        }
    }

    init {
        activity.application.registerComponentCallbacks(this)
    }

    fun attach(target: WebView) {
        if (destroyed || webView === target) return
        detachView()
        attachment += 1
        webView = target
        target.addOnLayoutChangeListener(layoutListener)
        target.addOnAttachStateChangeListener(attachListener)
        if (target.isAttachedToWindow) installDrawObserver()
    }

    /** Called before Wry invokes WebView.onPause; PixelCopy reads the queued Window surface. */
    fun onPausing() {
        if (destroyed || paused) return
        paused = true
        resumed = false
        cycle += 1
        cancelDrawWork()
        cancelTimeout()
        // Never capture our own cover. A second background transition may keep
        // the one frame that is already being displayed during recovery.
        if (cover != null) {
            if (snapshot == null) {
                removeCover()
                token = null
                pendingReady = null
                memorySuppressed = true
            }
            return
        }
        returning = false
        memorySuppressed = false
        releaseSnapshot()
        val target = webView ?: return
        if (!target.isAttachedToWindow || !target.isShown || !hasDrawn || capture != null) return
        val rect = windowRect(target) ?: return
        val bitmap = try {
            Bitmap.createBitmap(rect.width(), rect.height(), Bitmap.Config.ARGB_8888)
        } catch (error: OutOfMemoryError) {
            memorySuppressed = true
            Log.w(TAG, "Resume snapshot allocation unavailable", error)
            return
        }
        val pending = Capture(bitmap, attachment, cycle)
        capture = pending
        try {
            PixelCopy.request(activity.window, rect, bitmap, { result ->
                if (capture === pending) capture = null
                val wanted = pending.accepted && !destroyed && pending.attachment == attachment
                    && pending.cycle == cycle && (paused || returning || cover != null)
                if (result == PixelCopy.SUCCESS && wanted) {
                    snapshot = bitmap
                    snapshotDisplayed = false
                    cover?.let { installSnapshot(it) }
                } else {
                    // PixelCopy has finished writing. This bitmap was never displayed.
                    bitmap.recycle()
                    if (result != PixelCopy.SUCCESS && !destroyed) {
                        Log.w(TAG, "Resume PixelCopy did not produce a frame: $result")
                    }
                }
            }, main)
        } catch (error: Exception) {
            if (capture === pending) capture = null
            bitmap.recycle()
            Log.w(TAG, "Resume PixelCopy could not start", error)
        }
    }

    fun onStopped() {
        if (destroyed) return
        stopped = true
        resumed = false
        returning = true
        cancelDrawWork()
        cancelTimeout()
    }

    /** Install before the Activity resumes/draws; a pause without stop never reaches this path. */
    fun onStarted() {
        if (destroyed || !stopped) return
        stopped = false
        returning = true
        if (memorySuppressed) {
            returning = false
            return
        }
        ensureCover()
        beginHandshake()
    }

    fun onResumed() {
        if (destroyed) return
        paused = false
        resumed = true
        if (cover == null && returning && !memorySuppressed) {
            ensureCover()
            if (cover != null) beginHandshake()
        }
        if (cover == null) {
            returning = false
            capture?.accepted = false
            releaseSnapshot()
            return
        }
        // Application.onActivityResumed precedes Wry's WebView.onResume. Defer
        // all renderer work until that lifecycle stack has returned.
        val currentAttachment = attachment
        main.post {
            if (destroyed || attachment != currentAttachment || !resumed || cover == null) return@post
            updateGeometry()
            armTimeout()
            dispatchResume()
            pendingReady?.let(::startVisualBarrier)
        }
    }

    /** IPC acknowledges registration immediately; JavaScript validation stays native-owned. */
    fun connect(id: String): String? {
        val target = webView ?: return null
        val currentAttachment = attachment
        verifyMarker(target, id, null) { valid ->
            if (!valid || destroyed || webView !== target || attachment != currentAttachment) return@verifyMarker
            documentId = id
            dispatchResume()
        }
        return token
    }

    fun complete(id: String, requestedToken: String) {
        if (destroyed || cover == null || token != requestedToken) return
        val ready = Ready(id, requestedToken)
        // A document can acknowledge before the Activity's resume callback.
        pendingReady = ready
        if (resumed && !paused) startVisualBarrier(ready)
    }

    private fun startVisualBarrier(ready: Ready) {
        val target = webView ?: return
        if (!resumed || paused || cover == null || token != ready.token) return
        cancelDrawWork()
        val attempt = DrawAttempt(ready, attachment, serial)
        verifyMarker(target, ready.documentId, ready.token) { valid ->
            if (!isCurrent(attempt)) return@verifyMarker
            if (!valid) {
                beginHandshake()
                return@verifyMarker
            }
            documentId = ready.documentId
            try {
                target.postVisualStateCallback(attempt.serial, object : WebView.VisualStateCallback() {
                    override fun onComplete(requestId: Long) {
                        if (!isCurrent(attempt) || requestId != attempt.serial) return
                        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q && target.isHardwareAccelerated) {
                            // ViewRootImpl captures frame callbacks BEFORE it
                            // dispatches OnDrawListener. Register before invalidation.
                            awaitFrameCommit(target, attempt)
                        } else {
                            drawAttempt = attempt
                        }
                        // The callback promises the NEXT draw, not pixels on screen.
                        target.invalidate()
                        cover?.invalidate()
                    }
                })
            } catch (error: Exception) {
                Log.w(TAG, "Resume visual barrier unavailable", error)
                showRecoveryActions()
            }
        }
    }

    @android.annotation.TargetApi(Build.VERSION_CODES.Q)
    private fun awaitFrameCommit(target: WebView, attempt: DrawAttempt) {
        val observer = target.viewTreeObserver
        val callback = Runnable {
            main.post { if (isCurrent(attempt)) verifyAndReveal(target, attempt) }
        }
        try {
            if (!observer.isAlive) return
            frameObserver = observer
            frameCallback = callback
            observer.registerFrameCommitCallback(callback)
        } catch (error: Exception) {
            Log.w(TAG, "Resume frame commit unavailable", error)
            main.post { if (isCurrent(attempt)) showRecoveryActions() }
        }
    }

    private fun waitForDrawSubmission(target: WebView, attempt: DrawAttempt) {
        // API 26–28 (or non-hardware rendering): OnDrawListener runs BEFORE
        // drawing. Repost after traversal, then wait for the following frame;
        // never remove views/listeners from the OnDraw callback itself.
        val after = Runnable {
            if (!isCurrent(attempt)) return@Runnable
            val frame = Runnable { if (isCurrent(attempt)) verifyAndReveal(target, attempt) }
            nextFrame = frame
            target.postOnAnimation(frame)
        }
        afterDraw = after
        main.post(after)
    }

    private fun verifyAndReveal(target: WebView, attempt: DrawAttempt) {
        verifyMarker(target, attempt.ready.documentId, attempt.ready.token) { valid ->
            if (!isCurrent(attempt)) return@verifyMarker
            if (!valid) {
                beginHandshake()
                return@verifyMarker
            }
            capture?.accepted = false
            returning = false
            cancelDrawWork()
            cancelTimeout()
            token = null
            pendingReady = null
            removeCover()
            releaseSnapshot()
            dispatchFinished(target, attempt.ready)
        }
    }

    private fun isCurrent(attempt: DrawAttempt): Boolean = !destroyed && resumed && !paused
        && webView?.isAttachedToWindow == true && webView?.isShown == true
        && webView?.windowVisibility == View.VISIBLE
        && cover != null && attachment == attempt.attachment && serial == attempt.serial
        && token == attempt.ready.token

    private fun verifyMarker(target: WebView, id: String, readyToken: String?, completion: (Boolean) -> Unit) {
        val check = if (readyToken == null) "true" else "m.readyToken===${JSONObject.quote(readyToken)}"
        val script = "(function(){var m=window.__vcpWebViewResume;return !!m&&m.documentId===${JSONObject.quote(id)}&&($check);})()"
        try {
            target.evaluateJavascript(script) { result -> completion(result == "true") }
        } catch (error: Exception) {
            Log.w(TAG, "Resume document validation unavailable", error)
            completion(false)
        }
    }

    private fun dispatchResume() {
        val target = webView ?: return
        val id = documentId ?: return
        val currentToken = token ?: return
        if (!resumed || paused || cover == null) return
        val script = "(function(){var m=window.__vcpWebViewResume;if(m&&m.documentId===${JSONObject.quote(id)})window.dispatchEvent(new CustomEvent('vcp-webview-resume',{detail:{token:${JSONObject.quote(currentToken)}}}));})()"
        try { target.evaluateJavascript(script, null) }
        catch (error: Exception) { Log.w(TAG, "Resume readiness event unavailable", error) }
    }

    private fun dispatchFinished(target: WebView, ready: Ready) {
        val script = "(function(){var m=window.__vcpWebViewResume;if(m&&m.documentId===${JSONObject.quote(ready.documentId)})window.dispatchEvent(new CustomEvent('vcp-webview-resume',{detail:{token:${JSONObject.quote(ready.token)},finished:true}}));})()"
        try { target.evaluateJavascript(script, null) }
        catch (error: Exception) { Log.w(TAG, "Resume completion event unavailable", error) }
    }

    private fun beginHandshake() {
        if (destroyed || cover == null) return
        cancelDrawWork()
        cancelTimeout()
        token = UUID.randomUUID().toString()
        pendingReady = null
        cover?.showWaiting(snapshot != null)
        if (resumed && !paused) {
            armTimeout()
            dispatchResume()
        }
    }

    private fun armTimeout() {
        cancelTimeout()
        val currentToken = token ?: return
        val task = Runnable {
            if (token == currentToken && resumed && !paused && cover != null) showRecoveryActions()
        }
        timeout = task
        main.postDelayed(task, 5_000)
    }

    private fun showRecoveryActions() {
        cover?.showRecovery()
    }

    private fun ensureCover() {
        // Without captured pixels, preserve the framework's normal rendering.
        // Never substitute a full-screen color/loading panel for the old frame.
        if (snapshot == null) return
        val target = webView ?: return
        val host = activity.window.decorView as? ViewGroup ?: return
        val rect = windowRect(target) ?: return
        if (cover == null) {
            val created = ResumeCoverView(activity, ::beginHandshake) {
                documentId = null
                beginHandshake()
                try { webView?.reload() }
                catch (error: Exception) { Log.w(TAG, "Explicit interface reload failed", error) }
            }
            cover = created
            previousAccessibility = target.importantForAccessibility
            target.importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_NO_HIDE_DESCENDANTS
            host.addView(created, FrameLayout.LayoutParams(rect.width(), rect.height()))
            created.bringToFront()
            installSnapshot(created)
        }
        positionCover(rect)
    }

    private fun installSnapshot(target: ResumeCoverView) {
        val bitmap = snapshot ?: return
        if (target !== cover) return
        target.image.setImageBitmap(bitmap)
        snapshotDisplayed = true
        target.snapshotInstalled()
    }

    private fun windowRect(target: WebView): Rect? {
        if (!target.isAttachedToWindow || target.width <= 0 || target.height <= 0) return null
        val location = IntArray(2)
        target.getLocationInWindow(location)
        return Rect(location[0], location[1], location[0] + target.width, location[1] + target.height)
    }

    private fun positionCover(rect: Rect) {
        val current = cover ?: return
        val host = current.parent as? View ?: return
        val origin = IntArray(2)
        host.getLocationInWindow(origin)
        current.layoutParams = FrameLayout.LayoutParams(rect.width(), rect.height()).apply {
            leftMargin = rect.left - origin[0]
            topMargin = rect.top - origin[1]
        }
        coveredRect = Rect(rect)
    }

    private fun updateGeometry() {
        if (cover == null) {
            if (returning && !memorySuppressed && resumed && !paused) {
                ensureCover()
                if (cover != null) beginHandshake()
            }
            return
        }
        val rect = webView?.let(::windowRect) ?: return
        if (rect == coveredRect) return
        positionCover(rect)
        // Keep and temporarily fit the one old frame; validate the new geometry.
        beginHandshake()
    }

    private fun installDrawObserver() {
        removeDrawObserver()
        val observer = webView?.viewTreeObserver ?: return
        if (observer.isAlive) {
            drawObserver = observer
            observer.addOnDrawListener(drawListener)
        }
    }

    private fun removeDrawObserver() {
        drawObserver?.let { if (it.isAlive) it.removeOnDrawListener(drawListener) }
        drawObserver = null
    }

    private fun cancelDrawWork() {
        serial += 1
        drawAttempt = null
        afterDraw?.let(main::removeCallbacks)
        afterDraw = null
        nextFrame?.let { webView?.removeCallbacks(it) }
        nextFrame = null
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            frameCallback?.let { callback -> frameObserver?.let { if (it.isAlive) it.unregisterFrameCommitCallback(callback) } }
        }
        frameObserver = null
        frameCallback = null
    }

    private fun cancelTimeout() {
        timeout?.let(main::removeCallbacks)
        timeout = null
    }

    private fun removeCover() {
        cover?.let {
            it.image.setImageDrawable(null)
            (it.parent as? ViewGroup)?.removeView(it)
        }
        cover = null
        coveredRect = null
        previousAccessibility?.let { webView?.importantForAccessibility = it }
        previousAccessibility = null
    }

    private fun releaseSnapshot() {
        val bitmap = snapshot
        snapshot = null
        // A displayed image can still belong to RenderThread's old display list.
        // Dropping strong references lets Android reclaim it after that use ends.
        if (!snapshotDisplayed) bitmap?.recycle()
        snapshotDisplayed = false
    }

    private fun detachView() {
        cancelDrawWork()
        cancelTimeout()
        capture?.accepted = false
        removeCover()
        releaseSnapshot()
        removeDrawObserver()
        webView?.removeOnLayoutChangeListener(layoutListener)
        webView?.removeOnAttachStateChangeListener(attachListener)
        webView = null
        token = null
        pendingReady = null
        documentId = null
        hasDrawn = false
    }

    fun destroy() {
        if (destroyed) return
        destroyed = true
        attachment += 1
        detachView()
        activity.application.unregisterComponentCallbacks(this)
        // An in-flight PixelCopy alone retains its destination until its callback.
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        main.post { if (!destroyed) updateGeometry() }
    }

    override fun onLowMemory() = releaseForMemoryPressure()

    override fun onTrimMemory(level: Int) {
        // UI_HIDDEN is a lifecycle notice, not a request to drop every background snapshot.
        if (level != ComponentCallbacks2.TRIM_MEMORY_UI_HIDDEN && level >= ComponentCallbacks2.TRIM_MEMORY_RUNNING_LOW) {
            releaseForMemoryPressure()
        }
    }

    private fun releaseForMemoryPressure() {
        main.post {
            if (destroyed) return@post
            capture?.accepted = false
            cover?.image?.setImageDrawable(null)
            releaseSnapshot()
            if (!resumed || paused) {
                memorySuppressed = true
                returning = false
                cancelDrawWork()
                cancelTimeout()
                token = null
                pendingReady = null
                removeCover()
            } else if (cover != null) {
                cover?.showRecovery()
            }
        }
    }

    private companion object { const val TAG = "VCPWebViewResume" }
}
