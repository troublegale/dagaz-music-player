package io.dagaz.player

import java.io.File
import org.junit.Assume.assumeFalse

fun requireEmptyConnectionFixture(file:File){assumeFalse("Existing personal connection is preserved",listOf("",".bak",".new").any{File(file.path+it).exists()})}
