package io.dagaz.player.downloads

import io.dagaz.player.api.Quality

fun reuseDownload(existing:Quality,requested:Quality,force:Boolean=false):Boolean =
    if(force)existing==requested else existing.ordinal<=requested.ordinal
