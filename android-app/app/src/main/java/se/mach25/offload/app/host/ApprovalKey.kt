package se.mach25.offload.app.host

import android.app.Activity
import android.content.Context
import android.hardware.biometrics.BiometricManager
import android.hardware.biometrics.BiometricPrompt
import android.os.Build
import android.os.CancellationSignal
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyInfo
import android.security.keystore.KeyProperties
import android.security.keystore.StrongBoxUnavailableException
import android.util.Log
import org.json.JSONObject
import java.io.File
import java.security.KeyFactory
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.Signature
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec

/**
 * The fleet approval key in secure hardware (ADR-0069 §4).
 *
 * A P-256 key in the Android Keystore — StrongBox where the device has one, the TEE otherwise —
 * that cannot be exported and signs only after the person at the device confirms, every time.
 * The daemon never holds it: it files a request in `sign-requests/`, this signs it, and the answer
 * goes back the same way. The public key and the security level Android verified go in
 * `approval-key.json` for `offload grant approve --hardware-key` to name.
 */
object ApprovalKey {
    private const val TAG = "offload-approval"
    private const val ALIAS = "offload-approval"

    fun keyFile(c: Context) = File(Paths.stateDir(c), "approval-key.json")
    fun requestsDir(c: Context) = File(Paths.stateDir(c), "sign-requests")

    private fun keyStore(): KeyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    fun exists(): Boolean = keyStore().containsAlias(ALIAS)

    /** Make the key if there is none, and (re)write `approval-key.json`. Returns what it says. */
    fun ensure(c: Context): String {
        if (!exists()) {
            try {
                generate(strongBox = true)
            } catch (e: StrongBoxUnavailableException) {
                generate(strongBox = false)
            }
        }
        val entry = keyStore().getEntry(ALIAS, null) as KeyStore.PrivateKeyEntry
        val public = entry.certificate.publicKey as ECPublicKey
        val level = securityLevel(entry.privateKey)
        val sec1 = uncompressed(public)
        Paths.stateDir(c).mkdirs()
        val json = JSONObject().put("sec1", sec1.toHex()).put("security_level", level)
        val tmp = File(Paths.stateDir(c), "approval-key.json.tmp")
        tmp.writeText(json.toString())
        tmp.renameTo(keyFile(c))
        return "approval key: P-256, $level"
    }

    private fun generate(strongBox: Boolean) {
        val spec = KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_SIGN)
            .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
            .setDigests(KeyProperties.DIGEST_SHA256)
            .setUserAuthenticationRequired(true)
            .apply {
                // Every use, not a window after unlocking: an approval is a person's act.
                if (Build.VERSION.SDK_INT >= 30) {
                    setUserAuthenticationParameters(
                        0,
                        KeyProperties.AUTH_BIOMETRIC_STRONG or KeyProperties.AUTH_DEVICE_CREDENTIAL,
                    )
                }
                if (strongBox) setIsStrongBoxBacked(true)
            }
            .build()
        KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore").apply {
            initialize(spec)
            generateKeyPair()
        }
    }

    /** As Android verified it, which is the only thing this report may claim. */
    private fun securityLevel(key: PrivateKey): String {
        val info = KeyFactory.getInstance(key.algorithm, "AndroidKeyStore")
            .getKeySpec(key, KeyInfo::class.java)
        return if (Build.VERSION.SDK_INT >= 31) {
            when (info.securityLevel) {
                KeyProperties.SECURITY_LEVEL_STRONGBOX -> "STRONGBOX"
                KeyProperties.SECURITY_LEVEL_TRUSTED_ENVIRONMENT -> "TRUSTED_ENVIRONMENT"
                KeyProperties.SECURITY_LEVEL_SOFTWARE -> "SOFTWARE"
                else -> "UNKNOWN"
            }
        } else {
            @Suppress("DEPRECATION")
            if (info.isInsideSecureHardware) "TRUSTED_ENVIRONMENT" else "SOFTWARE"
        }
    }

    private fun uncompressed(key: ECPublicKey): ByteArray {
        fun fixed(n: java.math.BigInteger): ByteArray {
            val raw = n.toByteArray().dropWhile { it == 0.toByte() }.toByteArray()
            return ByteArray(32 - raw.size) + raw
        }
        return byteArrayOf(0x04) + fixed(key.w.affineX) + fixed(key.w.affineY)
    }

    /** Requests waiting for an answer, oldest first. */
    fun pending(c: Context): List<File> =
        requestsDir(c).listFiles { f -> f.name.endsWith(".json") }
            ?.filter { req ->
                val id = req.name.removeSuffix(".json")
                !File(req.parentFile, "$id.sig").exists() && !File(req.parentFile, "$id.refused").exists()
            }
            ?.sortedBy { it.lastModified() }
            ?: emptyList()

    /**
     * Ask the person, and sign on a confirmed touch.
     *
     * The prompt is worded from the certificate in the request, and the bytes signed are computed
     * from the same certificate by the bundled `offload signing-bytes` — so what the person reads
     * and what is signed cannot differ, and there is no second implementation of the format here.
     */
    fun answer(activity: Activity, request: File, done: () -> Unit) {
        val id = request.name.removeSuffix(".json")
        val dir = request.parentFile ?: return
        fun refuse(reason: String) {
            File(dir, "$id.refused").writeText(reason)
            done()
        }
        val body = try {
            JSONObject(request.readText())
        } catch (e: Exception) {
            refuse("unreadable request: $e")
            return
        }
        val cert = body.optJSONObject("certificate") ?: run {
            refuse("the request carries no certificate")
            return
        }
        // Only the wording: what is signed is the certificate, whatever the request calls itself.
        val reapproval = body.optString("purpose") == "reapprove"
        val bytes = signingBytes(activity, cert.toString()) ?: run {
            refuse("could not compute the signing bytes")
            return
        }
        val name = cert.optString("name")
        // A node id is serialised as its 32 bytes; the first four, in hex, are what `offload nodes`
        // prints as the short form.
        val member = cert.optJSONArray("member")
            ?.let { a -> (0 until minOf(4, a.length())).joinToString("") { "%02x".format(a.getInt(it)) } }
            ?: "?"
        val grants = cert.optJSONArray("grants")
            ?.let { a -> (0 until a.length()).joinToString(", ") { a.getString(it) } }
            ?: ""
        val signature = try {
            Signature.getInstance("SHA256withECDSA").apply {
                initSign((keyStore().getEntry(ALIAS, null) as KeyStore.PrivateKeyEntry).privateKey)
            }
        } catch (e: Exception) {
            refuse("the approval key is not usable: $e")
            return
        }
        val prompt = BiometricPrompt.Builder(activity)
            .setTitle(if (reapproval) "Re-approve a device for another year?" else "Approve a device into your fleet?")
            .setSubtitle("$name ($member)")
            .setDescription("Grants: $grants")
            .apply {
                if (Build.VERSION.SDK_INT >= 30) {
                    setAllowedAuthenticators(
                        BiometricManager.Authenticators.BIOMETRIC_STRONG or
                            BiometricManager.Authenticators.DEVICE_CREDENTIAL,
                    )
                }
            }
            .build()
        // Anything that stops the prompt appearing is a refusal the waiting command can read, never
        // a crash: an uncaught SecurityException here took the whole app down on the tablet.
        try {
            prompt.authenticate(
            BiometricPrompt.CryptoObject(signature),
            CancellationSignal(),
            activity.mainExecutor,
            object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                    try {
                        val s = result.cryptoObject?.signature ?: run {
                            refuse("the prompt returned no signature object")
                            return
                        }
                        s.update(bytes)
                        val der = s.sign()
                        val tmp = File(dir, "$id.sig.tmp")
                        tmp.writeText(der.toHex())
                        tmp.renameTo(File(dir, "$id.sig"))
                        done()
                    } catch (e: Exception) {
                        Log.w(TAG, "signing", e)
                        refuse("signing failed: $e")
                    }
                }

                override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                    refuse(errString.toString())
                }
            },
        )
        } catch (e: Exception) {
            Log.w(TAG, "prompt", e)
            refuse("the prompt could not be shown: $e")
        }
    }

    private fun signingBytes(c: Context, certJson: String): ByteArray? =
        try {
            val p = ProcessBuilder(Paths.offload(c).path, "signing-bytes").redirectErrorStream(false).start()
            p.outputStream.use { it.write(certJson.toByteArray()) }
            val hex = p.inputStream.bufferedReader().readText().trim()
            if (p.waitFor() != 0 || hex.isEmpty()) null else hex.fromHex()
        } catch (e: Exception) {
            Log.w(TAG, "signing-bytes", e)
            null
        }

    private fun ByteArray.toHex() = joinToString("") { "%02x".format(it) }
    private fun String.fromHex() = chunked(2).map { it.toInt(16).toByte() }.toByteArray()
}
