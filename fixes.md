# Audit & Fixes

## Zastosowane poprawki

### 1. `src/cli.rs` — usunięty nieużywany import
```diff
- use std::cmp::Ord;
```
`Ord` był importowany ale nie używany; `#[derive(Ord)]` nie wymaga importu.

### 2. `src/region_loader/get_u32.rs` — dodany `debug_assert!`
`get_u32` używał `.unwrap()` bez sprawdzania granic. Dodano `debug_assert!` który wyłapie błędne wywołanie w buildach dev/test, nie kosztując nic w release.

### 3. `src/region_loader/region.rs` — refaktor `to_bytes`
**Problem:** `to_bytes(&mut self)` mutowało `self` jako efekt uboczny (liczniki `compression_fallbacks` i `header_write_failures`), co wymuszało `#[allow(clippy::wrong_self_convention)]`.

**Naprawa:**
- Usunięto pola `compression_fallbacks` i `header_write_failures` z `Region`
- `to_bytes` zmienione na `&self` i zwraca `ToBytesResult { bytes, compression_fallbacks, header_write_failures }`
- Usunięto getter `get_compression_fallbacks()` i `get_header_write_failures()`
- Usunięto suppression `#[allow(clippy::wrong_self_convention)]`

### 4. `src/region_loader/region.rs` — `&Vec<Chunk>` → `&[Chunk]`
`get_chunks()` zwracało `&Vec<Chunk>`. Clippy lintuje to: funkcja powinna zwracać `&[Chunk]`.

### 5. `src/region_loader/region.rs` + `read.rs` — `&PathBuf` → `&Path`
`from_file_name` przyjmowało `&PathBuf`. Zmienione na `&Path` (bardziej idiomatyczne, akceptuje każdą ścieżkę).

### 6. `src/nbt/tag.rs` — `find_tag(name: impl ToString)` → `find_tag(name: &str)`
Poprzednia sygnatura allokowała `String` przy każdym wywołaniu (`name.to_string()`). Zmienione na `&str` + `as_deref()` — zero alokacji.

### 7. `src/main.rs` + `Cargo.toml` — usunięta zależność `num_cpus`
`num_cpus::get()` zastąpione przez `std::thread::available_parallelism()` (stabilne od Rust 1.59, projekt wymaga 1.85). Usunięto crate `num_cpus` z `Cargo.toml`.

### 8. `src/commands/write.rs` — zaktualizowany do `ToBytesResult`
Dostosowany do nowego API `to_bytes` zwracającego struct zamiast mutowania region.

### 9. `Cargo.toml` — bump dependencji do najnowszych wersji (2026-05-13)
Wersje sprawdzone przez `crates.io` API:
- `clap` 4.5.53 → **4.6.1**
- `flate2` 1.1.5 → **1.1.9**
- `indicatif` 0.18.3 → **0.18.4**
- `rayon` 1.11.0 → **1.12.0**
- `thiserror` 2.0.17 → **2.0.18**
- `lz4_flex` 0.12.0 → **0.13.1** (brak breaking changes; API `frame::FrameDecoder` i `block::decompress_size_prepended` bez zmian)

### 10. `src/commands/read.rs` + `write.rs` — `&PathBuf` → `&Path` w `optimize_read/write`
Nowy clippy (Rust 1.95) lintuje sygnatury `fn(&PathBuf)`. Zmienione na `&Path` (zgodne z resztą kodu po fix #5).

### 11. `src/nbt/parse.rs` — `excessive_precision` w literałach `f32`/`f64`
Stałe testowe `0.498_231_470_584_869_38_f32` i `0.493_128_713_218_231_48_f64` miały precyzję większą niż reprezentowalna. Skrócone do `0.498_231_47_f32` i `0.493_128_713_218_231_5_f64` — wartości binarne identyczne (clippy::excessive_precision).

### 12. `src/commands/write.rs` — atomic write przez `tempfile + rename`
**Problem:** poprzedni kod robił `File::create(region_file_path)` → truncate → `write_all`. Jeśli proces zostałby zabity / crash / power loss w trakcie pisania pojedynczego `.mca`, ten plik byłby uszkodzony i nieodzyskiwalny. Krytyczne dla świata 800 GB gdzie operacja trwa godziny.

**Fix:** każdy region jest teraz pisany w schemacie:
1. `File::create(region_file_path.mca.tmp.<pid>.<thread_id>)` — sibling tempfile w tym samym katalogu (krytyczne: `rename` musi być w obrębie jednego filesystem żeby było atomic)
2. `write_all` + `flush`
3. `std::fs::rename(tmp, region_file_path)` — atomic na POSIX (Linux/macOS), używa `ReplaceFile` na Windows
4. W przypadku błędu na dowolnym kroku: tempfile jest sprzątany przez `remove_file`

**Peak dyskowy:** `stary_rozmiar + nowy_rozmiar` tylko dla regionów aktualnie pisanych równolegle. Region MCA = 4–10 MB, ~16 wątków rayon = ~150 MB peak overhead. Po operacji peak znika. To NIE jest backup całego świata.

**Test:** `test_optimize_write_atomic_on_real_sample` — kopiuje realny 11 MB sample do tmp dir, uruchamia `optimize_write`, re-parsuje wynik, weryfikuje że nie zostały żadne pliki `*.tmp.*`.

### 13. `src/region_loader/region.rs` — test `test_roundtrip_decompressed_nbt_byte_for_byte`
Nowy unit test który dla każdego chunka w realnym sample `test_files/r.-1.-1.mca` (11 MB, ~1000 chunków):
1. Parsuje oryginał → `original_region`
2. `to_bytes(compression)` → bajty zapisane
3. Re-parsuje → `parsed_region`
4. Porównuje **zdekompresowany NBT bajt-w-bajt** (`nbt.to_bytes()`)
5. Powtarza dla `Compression::fast()`, `default()`, `best()`

Sprawdza dodatkowo `compression_fallbacks == 0` i `header_write_failures == 0`. Nie porównujemy surowych skompresowanych bajtów — zlib-ng z innym poziomem produkuje legalnie różne strumienie deflate, ale zdekompresowana zawartość (czyli to co czyta Minecraft) musi być identyczna.

**Pokrycie:** zlib (scheme 2) — dominujące w sample. LZ4 (scheme 4, Minecraft 24w04a+) nie występuje w tym pliku; round-trip dla LZ4 sprawdzany jest pośrednio przez parser w `chunk.rs:66-78`. Sample LZ4 region warto dodać gdy będzie dostępny.

---

## Pozostałe znane problemy (nienaprawione)

### A. ~~`Cargo.toml` — `flate2` z `zlib-ng` wymaga `cmake`~~ **NAPRAWIONE**
cmake zainstalowany lokalnie przez `brew install cmake` (`cmake version 4.3.2`). Build z `zlib-ng` działa.

### B. `src/region_loader/region.rs:53` — FIXME: utrata chunków z nieobsługiwanymi schematami kompresji
```rust
// FIXME: We might not want to loose the chunk if the compression scheme is an unsupported type
// (eg. LZ4 since 24w04a or custom algorithm since 24w05a)
```
Chunki z nieznanym bajtem schematu kompresji są pomijane (nie wczytywane), co oznacza ich usunięcie. LZ4 (scheme byte `3`) jest już obsługiwany. Scheme `4` (custom/external, Minecraft 24w05a+) nie jest. Naprawa wymagałaby zachowania oryginalnych bajtów nawet gdy kompresja jest nieobsługiwana.

### C. `src/nbt/binary_reader.rs:78` — `read_name()` zwraca `None` dla pustego stringa
```rust
pub fn read_name(&mut self) -> Option<String> {
    self.read_string().ok().filter(|s| !s.is_empty())
}
```
Kolapuje dwa różne stany: błąd odczytu i pusty string `""`. W praktyce chunki Minecraft zawsze mają nazwy, więc nie jest to realny problem.

### D. Cicha utrata błędów w przetwarzaniu równoległym
```rust
.filter_map(|entry| {
    let result = optimize_write(entry, compression);
    pb.inc(1);
    result.ok()  // błędy io_errors zliczane wewnątrz, ale Ok() zawsze zwracane
})
```
`optimize_read/write` zawsze zwraca `Ok(result)` (błędy I/O zliczane w struct), więc `.ok()` nigdy nie filtruje — kod jest poprawny ale mylący. Można by zmienić zwracany typ na `OptimizeResult` zamiast `io::Result<OptimizeResult>`.

---

## Weryfikacja

Aby zweryfikować poprawki po zainstalowaniu cmake:
```bash
cargo check          # brak błędów
cargo clippy -- -D warnings  # brak warningów
cargo test           # wszystkie testy przechodzą
```
