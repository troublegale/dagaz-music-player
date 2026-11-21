package io.dagaz.player.auth

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import android.util.Base64
import io.dagaz.player.api.*
import org.json.JSONObject
import java.io.File
import java.io.FileNotFoundException
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

class Credentials(context: Context, namespace: String = "connection") {
    init { require(namespace.matches(Regex("[a-zA-Z0-9-]{1,80}"))) }
    private val file = AtomicFile(File(context.noBackupFilesDir, "$namespace.json"))
    private val alias = "dagaz.$namespace"
    private fun store() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
    private fun key(create: Boolean): SecretKey {
        val existing = store().getKey(alias, null)
        if (existing is SecretKey) return existing
        if (!create) throw ClientException(Failure.PairingRequired)
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
            init(KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256).build())
        }.generateKey()
    }
    @Synchronized fun save(session: Session) {
        var stream: java.io.FileOutputStream? = null
        try {
            ServerAddress.parse(session.baseUrl)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            try { cipher.init(Cipher.ENCRYPT_MODE, key(true)) } catch (_: KeyPermanentlyInvalidatedException) {
                store().deleteEntry(alias)
                cipher.init(Cipher.ENCRYPT_MODE, key(true))
            }
            // Authenticate the origin and device ID together with the token, using a fresh IV each save.
            val encrypted = cipher.doFinal(Wire.json.encodeToString(Session.serializer(), session).toByteArray(Charsets.UTF_8))
            val envelope = JSONObject().put("version", 1).put("iv", Base64.encodeToString(cipher.iv, Base64.NO_WRAP)).put("ciphertext", Base64.encodeToString(encrypted, Base64.NO_WRAP))
            stream = file.startWrite()
            stream.write(envelope.toString().toByteArray(Charsets.UTF_8))
            file.finishWrite(stream)
        } catch (_: Exception) {
            if (stream != null) file.failWrite(stream)
            throw ClientException(Failure.Storage)
        }
    }
    @Synchronized fun load(): Session? {
        val bytes = try { file.openRead().use { input ->
            val buffer = ByteArray(65537); var used = 0
            while (used < buffer.size) { val count = input.read(buffer, used, buffer.size - used); if (count < 0) break; used += count }
            if (used > 65536) throw ClientException(Failure.PairingRequired)
            buffer.copyOf(used)
        } } catch (_: FileNotFoundException) { return null }
        catch (_: java.io.IOException) { throw ClientException(Failure.Storage) }
        try {
            val envelope = JSONObject(bytes.toString(Charsets.UTF_8))
            require(envelope.getInt("version") == 1)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            val iv = Base64.decode(envelope.getString("iv"), Base64.NO_WRAP)
            require(iv.size == 12)
            cipher.init(Cipher.DECRYPT_MODE, key(false), GCMParameterSpec(128, iv))
            val plain = cipher.doFinal(Base64.decode(envelope.getString("ciphertext"), Base64.NO_WRAP))
            val session = Wire.json.decodeFromString<Session>(plain.toString(Charsets.UTF_8))
            ServerAddress.parse(session.baseUrl)
            return session
        } catch (_: Exception) { throw ClientException(Failure.PairingRequired) }
    }
}
