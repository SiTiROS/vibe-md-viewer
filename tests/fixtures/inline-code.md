# Отчёт проверки: код в тексте

## 128. `CSV_HEADER` пишет вводящий в заблуждение `column=` в итоговой записи

**Статус.** Актуально.

`src/demo_linter/checks/CsvHeaderCheck.cpp:77-158`

```cpp
std::string Column;   // :12
Names.push_back({ Field.Name(), std::format("col[{}]", I) });   // :77
Report.Info(std::format("CSV_HEADER: verified {} column name(s), sequence 001..{:03}; column={}; generated_columns={}. File: {}", Count, Last, Names[0].Column, Generated, Path));   // :155
                        Names[0].Column,   // :158
```

**Доказательство.** `HeaderName::Column` заполняется в одном месте — строкой `col[<индекс столбца>]` (`CsvHeaderCheck.cpp:77`); другого источника имён в проверке нет. В Error по отдельному имени (`:109`, `:121`, `:131`) это место ошибки — по делу. Итоговый Info про весь набор берёт `Names[0].Column` (`:158`) и пишет `column=col[0]`, а если перед названными столбцами стоят сгенерированные — `column=col[<индекс первого названного>]`: это читается как «имена взяты из столбца 0», хотя проверены все столбцы файла.

**Воспроизведение.** Временный файл, в заголовке которого два столбца с именами `id_<имя таблицы>_001` и `id_<имя таблицы>_002` (`CsvField::SetName`): `RunCheck` даёт `Info|CSV_HEADER: verified 2 column name(s), sequence 001..002; column=col[0]; generated_columns=0. File: <путь файла>`; та же пара после одного сгенерированного столбца (`bIsGenerated = true`) — `column=col[1]; generated_columns=1`.

**Последствие.** Поле `column=` в итоговой записи не несёт сведений и сбивает при разборе отчёта.

**Исправление.** Убрать `column={}` и аргумент `Names[0].Column` из сообщения `CsvHeaderCheck.cpp:155-160`; поле `HeaderName::Column` (`:12`) переименовать в место столбца (например, `Location`) — оно нужно только сообщениям `:109`, `:121`, `:131`.

## 129. `config.exception_paths` принимает путь папки без имени файла: запись проходит проверку формата и молча не действует

**Статус.** Актуально.

`src/demo_linter/checks/PathConfigCheck.cpp:49-61`

```cpp
if (!Value.is_string() || !TryGetString(Value, Path) || !IsValidFilePath(Path) || Path.find('*') != std::string::npos)   // :52
Report.Error(std::format("PATH_CONFIG: exception_paths requires exact file paths, e.g. data/tables/users.csv. File: {}", File));   // :56
bConfirmedException |= EqualsIgnoreCase(FilePath, Path);   // :60
```

**Доказательство.** Формат записи проверяет `IsValidFilePath` (`:52`); строку без `.` он проверяет как имя папки. Список в тексте:

- проверка `IsValidFilePath` для `data/tables/users` проходит;
- сравнение `EqualsIgnoreCase(FilePath, Path)` с `data/tables/users.csv` — нет;
- итог: исключение `bConfirmedException` никогда не срабатывает.

> Цитата с кодом: вызов `RunCheck()` возвращает `CheckResult::Ok`, а флаг `bIsGenerated` не учитывается.

| Поле | Тип | Пример |
| --- | --- | --- |
| `Column` | `std::string` | `col[0]` |
| `Location` | `int` | `3` |
