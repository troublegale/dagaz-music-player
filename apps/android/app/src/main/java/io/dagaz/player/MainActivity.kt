package io.dagaz.player

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.SystemBarStyle
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.media3.common.util.UnstableApi
import io.dagaz.player.library.*

@UnstableApi
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge(
            statusBarStyle = SystemBarStyle.dark(android.graphics.Color.TRANSPARENT),
            navigationBarStyle = SystemBarStyle.dark(android.graphics.Color.TRANSPARENT)
        )
        setContent {
            MaterialTheme(colorScheme = darkColorScheme(
                primary = Color(0xFF9CE6C6), onPrimary = Color(0xFF14362C),
                background = Color(0xFF151B24), onBackground = Color(0xFFE8EDF3),
                surface = Color(0xFF1C2531), onSurface = Color(0xFFE8EDF3),
                onSurfaceVariant = Color(0xFF91A0B4), error = Color(0xFFF2B3A9)
            )) {
                val model: ConnectionViewModel = viewModel()
                val state by model.state.collectAsStateWithLifecycle()
                var settings by rememberSaveable { mutableStateOf(false) }
                val library=if(state.paired)viewModel<LibraryViewModel>(key="library-${state.connectionId}")else null
                var previous by remember { mutableStateOf<LibraryViewModel?>(null) }
                LaunchedEffect(state.paired,state.connectionId){if(!state.paired){previous?.deactivate();previous=null}else{previous=library}}
                if(state.paired&&!settings&&library!=null)LibraryScreen(library){settings=true}
                else {
                    BackHandler(enabled=state.paired){settings=false}
                    ConnectionScreen(state,model::address,model::deviceName,model::connect,model::retry,{previous?.deactivate();model.rebind()},{settings=false})
                }
            }
        }
    }
}
@Composable
private fun ConnectionScreen(
    state: ConnectionUiState, address: (String) -> Unit, deviceName: (String) -> Unit,
    connect: (String) -> Unit, retry: () -> Unit, rebind: () -> Unit, library:()->Unit
) {
    // Codes stay in memory and are never included in saved instance state or backups.
    var code by remember { mutableStateOf("") }
    LaunchedEffect(state.paired) { if (state.paired) code = "" }
    Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        Column(Modifier.fillMaxSize().safeDrawingPadding().imePadding().verticalScroll(rememberScrollState()).padding(24.dp), verticalArrangement = Arrangement.spacedBy(24.dp)) {
            Text("◇  dagaz", style = MaterialTheme.typography.headlineMedium, color = MaterialTheme.colorScheme.primary)
            Spacer(Modifier.height(12.dp))
            Card(Modifier.fillMaxWidth(), colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface)) {
                Column(Modifier.padding(24.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    if (state.paired) {
                        Text("Мой сервер", style = MaterialTheme.typography.headlineSmall)
                        Text(state.address, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        Button(onClick = retry, enabled = !state.busy, modifier = Modifier.fillMaxWidth()) { Text("Проверить подключение") }
                        TextButton(onClick = rebind, enabled = !state.busy, modifier = Modifier.fillMaxWidth()) { Text("Привязать заново") }
                        TextButton(onClick = library, modifier = Modifier.fillMaxWidth()) { Text("К библиотеке") }
                    } else {
                        Text("Подключение к серверу", style = MaterialTheme.typography.headlineSmall)
                        Text("Музыка хранится на твоём сервере. Подключи его к этому устройству.", color = MaterialTheme.colorScheme.onSurfaceVariant)
                        OutlinedTextField(state.address, address, label = { Text("Адрес сервера") }, placeholder = { Text("https://music.example.com") }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri), singleLine = true, enabled = !state.busy, modifier = Modifier.fillMaxWidth())
                        OutlinedTextField(code, { code = it }, label = { Text("Код привязки") }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password), singleLine = true, visualTransformation = PasswordVisualTransformation(), enabled = !state.busy, modifier = Modifier.fillMaxWidth())
                        OutlinedTextField(state.deviceName, deviceName, label = { Text("Имя устройства") }, singleLine = true, enabled = !state.busy, modifier = Modifier.fillMaxWidth())
                        Button(onClick = { connect(code) }, enabled = !state.busy, modifier = Modifier.fillMaxWidth()) { Text("Подключить") }
                    }
                    if (state.busy) LinearProgressIndicator(Modifier.fillMaxWidth())
                    Text(state.message, style = MaterialTheme.typography.bodyMedium, color = if (state.error) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
    }
}
