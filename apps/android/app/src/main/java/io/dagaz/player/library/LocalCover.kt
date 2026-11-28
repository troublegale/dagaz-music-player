package io.dagaz.player.library

import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import io.dagaz.player.api.Track
import io.dagaz.player.auth.Credentials
import io.dagaz.player.sync.LocalDatabase
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File

@Composable
fun LocalCover(track:Track) {
    val context=LocalContext.current
    var bitmap by remember(track.artworkId){mutableStateOf<android.graphics.Bitmap?>(null)}
    LaunchedEffect(track.artworkId){bitmap=withContext(Dispatchers.IO){val session=try{Credentials(context).load()}catch(_:Exception){null};if(session==null||track.artworkId==null)null else android.graphics.BitmapFactory.decodeFile(File(context.noBackupFilesDir,"covers-${LocalDatabase.scope(session)}/${track.artworkId}.png").path)}}
    val image=bitmap
    if(image!=null)Image(image.asImageBitmap(),contentDescription=null,modifier=Modifier.size(48.dp))else Text("♫")
}
