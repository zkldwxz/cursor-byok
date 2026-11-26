// Keeps the app awake out of sight while Cursor works on something, and tells what finished or waits.
package dev.cursorbyok.mobile

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import java.util.Locale

class Work : Service() {
  override fun onBind(intent: Intent?): IBinder? = null

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    val note = note(this, WORKING, "Cnod", intent?.getStringExtra(TEXT).orEmpty(), null)
    // Android freezes an app soon after it is left; only a foreground service keeps its connection open.
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
      startForeground(WORKING_ID, note, ServiceInfo.FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING)
    } else {
      startForeground(WORKING_ID, note)
    }
    running = true
    return START_NOT_STICKY
  }

  // Swiped away from the recent apps, the page that would end it is gone.
  override fun onTaskRemoved(rootIntent: Intent?) = stopSelf()

  override fun onDestroy() {
    running = false
    super.onDestroy()
  }

  companion object {
    const val TEXT = "text"
    const val SESSION = "session"
    private const val WORKING = "working"
    private const val CHATS = "chats"
    private const val WORKING_ID = 1
    const val CHAT_TAG = "chat"

    @Volatile
    var running = false
      private set

    private val chinese = Locale.getDefault().language == "zh"

    fun channels(context: Context) {
      context.getSystemService(NotificationManager::class.java).createNotificationChannels(listOf(
        NotificationChannel(WORKING, if (chinese) "正在处理" else "Working", NotificationManager.IMPORTANCE_LOW),
        NotificationChannel(CHATS, if (chinese) "对话" else "Chats", NotificationManager.IMPORTANCE_HIGH),
      ))
    }

    /** Shows the text of the running service again, without starting it from the background. */
    fun update(context: Context, text: String) {
      context.getSystemService(NotificationManager::class.java)
        .notify(WORKING_ID, note(context, WORKING, "Cnod", text, null))
    }

    /** One alert per conversation: a later one about it takes the place of the earlier. */
    fun tell(context: Context, session: String, title: String, text: String) {
      context.getSystemService(NotificationManager::class.java)
        .notify(CHAT_TAG, session.hashCode(), note(context, CHATS, title, text, session))
    }

    private fun note(context: Context, channel: String, title: String, text: String, session: String?): Notification {
      val open = Intent(context, MainActivity::class.java)
        .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
        .putExtra(SESSION, session)
      val touched = PendingIntent.getActivity(
        context, session?.hashCode() ?: 0, open,
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
      )
      return Notification.Builder(context, channel)
        .setSmallIcon(R.drawable.ic_alert)
        .setContentTitle(title)
        .setContentText(text)
        .setContentIntent(touched)
        .setAutoCancel(session != null)
        .setOngoing(session == null)
        .setCategory(if (session == null) Notification.CATEGORY_SERVICE else Notification.CATEGORY_MESSAGE)
        .build()
    }
  }
}
