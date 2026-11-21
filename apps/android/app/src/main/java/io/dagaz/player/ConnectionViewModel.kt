package io.dagaz.player

import android.app.Application
import android.os.Build
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import io.dagaz.player.api.*
import io.dagaz.player.auth.Credentials
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

data class ConnectionUiState(
    val address: String = "", val deviceName: String = Build.MODEL,
    val busy: Boolean = true, val paired: Boolean = false,
    val message: String = "Проверяем сохранённое подключение…", val error: Boolean = false
)
class ConnectionViewModel(application: Application) : AndroidViewModel(application) {
    private val credentials = Credentials(application)
    private val mutable = MutableStateFlow(ConnectionUiState())
    val state = mutable.asStateFlow()
    private var session: Session? = null
    init { execute {
        session = withContext(Dispatchers.IO) { credentials.load() }
        val saved = session
        if (saved == null) mutable.update { it.copy(message = "Введи адрес своего сервера и одноразовый код.") }
        else { mutable.update { it.copy(address = saved.baseUrl, paired = true) }; check(saved) }
    } }
    fun address(value: String) { if (!state.value.busy) mutable.update { it.copy(address = value) } }
    fun deviceName(value: String) { if (!state.value.busy) mutable.update { it.copy(deviceName = value) } }
    fun connect(code: String) {
        if (state.value.busy) return
        val address = state.value.address.trim(); val name = state.value.deviceName
        execute {
            val api = DagazApi(address)
            val paired = api.pair(code.trim(), name)
            val connection = Session(api.baseUrl, paired.deviceId, paired.token)
            withContext(Dispatchers.IO) { credentials.save(connection) }
            session = connection
            mutable.update { it.copy(address = connection.baseUrl, paired = true, message = "Подключено") }
        }
    }
    fun retry() { if (!state.value.busy) session?.let { saved -> execute { check(saved) } } }
    fun rebind() {
        if (state.value.busy) return
        session = null
        mutable.update { it.copy(paired = false, message = "Введи новый одноразовый код.", error = false) }
    }
    private suspend fun check(saved: Session) {
        DagazApi(saved.baseUrl).checkAccess(saved)
        mutable.update { it.copy(paired = true, message = "Подключено") }
    }
    private fun execute(operation: suspend () -> Unit) {
        mutable.update { it.copy(busy = true, error = false) }
        viewModelScope.launch {
            try { operation() }
            catch (error: ClientException) {
                if (error.failure == Failure.PairingRequired) session = null
                mutable.update { it.copy(paired = session != null, message = error.failure.label, error = true) }
            } finally { mutable.update { it.copy(busy = false) } }
        }
    }
}
