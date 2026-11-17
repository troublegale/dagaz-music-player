# Происхождение аудиофикстур

Короткие синтетические сигналы 440 Гц, 44100 Гц, длительность 0,25 с.
Это сгенерированные тестовые данные без сторонних музыкальных записей.

Инструмент: FFmpeg 6.1.1-3ubuntu5 в локальном Docker-образе из
experiments/metadata/Dockerfile.ffmpeg (Ubuntu 24.04).

Сборка: docker build -f experiments/metadata/Dockerfile.ffmpeg -t dagaz-audio-tools:validation experiments/metadata

Базовая команда (mount fixtures в /data):

```text
ffmpeg -f lavfi -i sine=frequency=440:sample_rate=44100:duration=0.25 -metadata title=Keep -metadata comment=DoNotChange -c:a CODEC OUTPUT
```

| Файл | CODEC / параметры |
| --- | --- |
| pcm.wav | pcm_s16le |
| float.wav | pcm_f32le |
| untagged.wav | pcm_s16le, без метаданных (`-map_metadata -1`) |
| vbr.mp3 | libmp3lame, `-q:a 4` |
| tone.flac | flac |
| unknown-chunk.wav | pcm.wav + RIFF-блок DAGZ с 9 байтами DoNotLose, выравниванием и обновлённым размером RIFF |

Обложки: PNG из FFmpeg lavfi color=c=blue:s=16x16 и 2048x2048, по одному кадру.
Большая обложка имеет 4 мегапикселя, но хорошо сжимается; это проверка размера изображения,
не лимита байтов HTTP-загрузки. Ошибочные файлы создаются непосредственно тестами во временном каталоге.
