package se.mach25.offload.app.ui

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import se.mach25.offload.app.R

/** The logo's colours (assets/app-icon.svg): the loop's cyan-to-violet, the arrow's orange, navy. */
object Brand {
    val Cyan = Color(0xFF22D3EE)
    val Blue = Color(0xFF4F7CFF)
    val Violet = Color(0xFF9B6CFF)
    val Orange = Color(0xFFFF7849)
    val Navy = Color(0xFF172554)
    val Ink = Color(0xFF0B1024)
    val Card = Color(0xFF131B3A)
    val CardHigh = Color(0xFF1B2550)

    /** The loop, as a brush: for the wordmark and anything that should read as "Offload". */
    val Loop = Brush.linearGradient(listOf(Cyan, Blue, Violet))

    /** The icon's background, top-left to bottom-right. */
    val Background = Brush.linearGradient(listOf(Navy, Ink))
}

// One scheme, the icon's: the app is the logo's navy whatever the system theme, so the mark and the
// screens behind it always belong together.
private val Scheme = darkColorScheme(
    primary = Brand.Cyan,
    onPrimary = Brand.Ink,
    primaryContainer = Brand.Navy,
    onPrimaryContainer = Brand.Cyan,
    secondary = Brand.Violet,
    onSecondary = Color.White,
    secondaryContainer = Color(0xFF2A2463),
    onSecondaryContainer = Color(0xFFE2D9FF),
    tertiary = Brand.Orange,
    onTertiary = Brand.Ink,
    background = Brand.Ink,
    onBackground = Color(0xFFE6E9F5),
    surface = Brand.Card,
    onSurface = Color(0xFFE6E9F5),
    surfaceVariant = Brand.CardHigh,
    onSurfaceVariant = Color(0xFFB4BBD6),
    surfaceContainer = Brand.Card,
    surfaceContainerHigh = Brand.CardHigh,
    surfaceContainerHighest = Brand.CardHigh,
    outline = Color(0xFF3A4570),
)

@Composable
fun OffloadTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = Scheme) {
        Box(Modifier.fillMaxSize().background(Brand.Background)) { content() }
    }
}

/** The mark and the wordmark, the loop's gradient on the name. */
@Composable
fun BrandBar() {
    Row(
        modifier = Modifier.fillMaxWidth().statusBarsPadding().padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Image(painterResource(R.drawable.ic_mark), contentDescription = null, modifier = Modifier.size(34.dp))
        Spacer(Modifier.size(10.dp))
        Text(
            "Offload",
            style = TextStyle(brush = Brand.Loop, fontSize = 26.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.5.sp),
        )
    }
}
