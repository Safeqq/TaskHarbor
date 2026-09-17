# AGENT.md — TaskHarbor

Panduan coding agent dan roadmap belajar bertahap. Status awal: perencanaan; belum ada fase yang dinyatakan selesai.

## 1. Tujuan dan cara menggunakan dokumen

TaskHarbor adalah web untuk membuat, menjadwalkan, menjalankan, dan memantau background job. Pengguna mengunggah gambar, memilih pengaturan, melihat progres pemrosesan, lalu mengunduh hasil. Backend utama memakai Rust. Proyek ditujukan untuk belajar dan portofolio software/backend engineer.

Dokumen ini khusus TaskHarbor dan berdiri sendiri. Jangan membawa modul incident monitoring atau Demo Shop dari proyek sebelumnya ke TaskHarbor.

Letakkan file ini di root repository TaskHarbor. Jika coding tool yang dipakai mengharapkan nama file instruksi lain, sesuaikan nama atau konfigurasinya; jangan menganggap `AGENT.md` selalu dimuat otomatis. Dokumen ini adalah instruksi proyek, bukan bukti bahwa folder atau command yang direncanakan sudah tersedia.

Instruksi pengguna saat ini dan instruksi dengan prioritas lebih tinggi tetap diutamakan. Pembuatan dokumen tidak mengotorisasi implementasi semua fase sekaligus.

## 2. Aturan pendampingan belajar untuk agent

- Gunakan bahasa Indonesia untuk penjelasan. Gunakan bahasa Inggris untuk identifier kode dan README portofolio.
- Default: mode berpasangan. Kerjakan satu sublangkah yang cukup kecil untuk dipahami dalam satu sesi, lalu jelaskan hasilnya. Jika pengguna meminta satu fase penuh atau beberapa fase, tuntaskan lingkup tersebut tanpa meminta izin berulang.
- Sebelum coding, sebutkan tujuan sublangkah, konsep yang dipelajari, dan perubahan yang akan dibuat. Jelaskan istilah baru saat pertama digunakan.
- Utamakan kode jelas dan alur nyata dibanding abstraksi generik. Jangan menambahkan framework atau dependency tanpa kebutuhan yang bisa dijelaskan.
- Setelah coding, jelaskan alur input → proses → output, keputusan penting, cara menjalankan, dan hasil verifikasi.
- Berikan satu latihan kecil serta 2–3 pertanyaan pemahaman. Latihan bersifat opsional, bukan penghalang bantuan berikutnya atau syarat izin melanjutkan tugas yang sudah diminta.
- Jangan melewati acceptance criteria teknis untuk mengejar roadmap. Boleh menjalankan fase independen jika prasyaratnya terpenuhi dan pengguna memintanya.
- Jangan membangun fitur fase lanjut secara diam-diam. Jika perubahan fondasi diperlukan, jelaskan alasannya dan jaga lingkupnya kecil.
- Saat pengguna mengatakan “lanjut”, baca checkpoint yang tersedia dan lanjutkan sublangkah berikutnya; jangan mulai ulang proyek atau mengarang kemajuan.
- Jangan menimpa perubahan pengguna. Periksa repository dan instruksi lokal sebelum bekerja; dokumentasikan hambatan konkret.
- Jangan mengaku test lulus atau fase selesai tanpa hasil nyata. Bedakan “kode selesai”, “terverifikasi”, dan “latihan pengguna belum dikerjakan”.

Ketika implementasi dimulai, buat `docs/progress.md` untuk checkpoint. Jangan membuat seluruh skeleton hanya untuk mengisi roadmap.

Format checkpoint:

```markdown
# Progress
Fase aktif: 0
Sublangkah berikutnya: 0.1
Status: not_started | in_progress | blocked | verified

## Implementasi selesai
- Perubahan dan file yang relevan

## Bukti verifikasi
- Command, hasil, dan tanggal; atau alasan belum dijalankan

## Catatan belajar
- Konsep yang dijelaskan
- Latihan opsional dan statusnya

## Keputusan dan keterbatasan
- Keputusan beserta alasan

## Langkah berikutnya
- Satu tugas kecil yang konkret
```

## 3. Produk dan batas fitur

### Fitur rilis pertama

- Satu akun pemilik; tanpa organisasi atau multi-tenant.
- Membuat job dan melihat daftar/detail beserta histori attempt.
- Pemrosesan nyata: resize gambar JPEG/PNG dan encode hasil menjadi JPEG dengan kualitas yang dipilih. Tangani transparansi PNG dengan latar eksplisit, misalnya putih.
- Satu job dapat memuat beberapa gambar, dengan progres berdasarkan item selesai.
- Mengunduh output individual. ZIP seluruh output menjadi tambahan setelah fitur inti stabil.
- Menjalankan segera, pada waktu tertentu, atau berulang dengan interval tetap.
- Prioritas antrean, pembatalan, retry terbatas, dan pemulihan setelah worker mati.
- Dashboard overview, jobs, schedules, dan workers.
- Login, validasi upload, dokumentasi, pengujian kegagalan, dan demo yang dapat diulang.

### Ditunda

Arbitrary code execution, shell command dari pengguna, cron expression, zona waktu rumit, workflow DAG, billing, multi-tenant, Redis/RabbitMQ, Kubernetes, dan worker lintas banyak mesin. Tipe job lain seperti PDF atau CSV ditambahkan setelah job gambar andal.

Kualitas JPEG lebih rendah tidak menjamin file selalu lebih kecil. UI harus menampilkan ukuran input/output nyata, format hasil, dan pengaturan yang dipakai.

## 4. Arsitektur dan alasan

Gunakan **modular monolith dengan executable API dan worker terpisah**, satu database PostgreSQL, serta React SPA. Scheduler awal adalah loop dalam executable worker. Tidak ada broker terpisah pada v1.

| Komponen | Peran |
|---|---|
| Dashboard React + TypeScript | Form job, progres, hasil, jadwal dan worker |
| Rust API: Axum + Tokio | Validasi HTTP, pengelolaan job, upload/download dan login |
| Rust worker | Mengklaim job, memproses gambar, melaporkan progres |
| Scheduler loop | Membuat job occurrence dari jadwal berulang |
| PostgreSQL + SQLx | Data job, antrean durable, attempt, lease, jadwal, session |
| Shared filesystem volume | Input dan output pada satu host |

Alasan: pengguna mempelajari satu sumber data, transaksi, dan concurrency dahulu; antrean database sudah cukup untuk produk awal. API/worker berbagi domain dan versi rilis. Database queue bukan otomatis jaminan exactly-once atau high availability.

Untuk pembagian kode, gunakan lapisan transport, application, domain, dan infrastructure. Handler HTTP tipis; aturan status berada di domain; aplikasi mengatur transaksi; SQLx dan filesystem menjadi adapter. Mulai dengan module Rust, pecah menjadi crate hanya ketika batasnya berguna. Frontend disusun berdasarkan fitur.

Struktur target, dibuat bertahap:

| Path | Isi |
|---|---|
| `apps/api/` | HTTP executable |
| `apps/worker/` | Worker dan scheduler |
| `apps/web/` | React dashboard |
| `crates/core/` | Job states, validasi, aturan scheduling |
| `crates/application/` | Use case dan kontrak I/O bila diperlukan |
| `crates/adapters/` | PostgreSQL, filesystem dan image processing |
| `migrations/` | Schema database |
| `tests/` | Integration dan failure scenarios |
| `deploy/` | Compose dan contoh environment |
| `docs/` | Progress, API, ADR, benchmark dan demo |

Jangan membuat dependency circular. Infrastructure mengimplementasikan kontrak yang digunakan application; domain tidak mengimpor Axum atau SQLx. Gunakan dependency/toolchain yang kompatibel, pin toolchain dan commit lockfiles. Jangan menebak versi library.

## 5. Kontrak job yang dijaga sepanjang implementasi

Kontrak akhir di bawah diperkenalkan sesuai fase. Keterbatasan fase awal harus tertulis, bukan disembunyikan sebagai fitur selesai.

### Status dan attempt

| Status internal | Arti |
|---|---|
| `queued` | Menunggu; eligible hanya jika `available_at <= now` |
| `running` | Sedang dimiliki attempt dengan lease yang berlaku |
| `retry_waiting` | Gagal sementara dan menunggu waktu retry |
| `cancel_requested` | Permintaan pembatalan sedang ditangani |
| `succeeded` | Hasil final telah dipublikasikan |
| `failed` | Gagal permanen atau usaha habis |
| `cancelled` | Pembatalan sudah diselesaikan |

“Scheduled” pada UI adalah job `queued` dengan waktu mendatang; “Offline” worker diturunkan dari heartbeat, bukan bukti bahwa proses pasti mati.

- Alur normal: `queued → running → succeeded`.
- Gagal sementara: `running → retry_waiting → running`; gagal permanen/limit: `running → failed`.
- Pembatalan job belum berjalan langsung menuju `cancelled`; job berjalan melalui `cancel_requested`.
- Terminal job tidak dibuka ulang. Manual retry membuat job baru dengan `retry_of_job_id`, sementara automatic retry menambah attempt pada job yang sama.
- `max_attempts` termasuk usaha pertama. Setiap usaha memiliki ID, nomor, waktu dan alasan kegagalan sendiri.
- Gunakan waktu UTC dan waktu database untuk keputusan eligibility/lease. UI boleh mengonversi ke waktu lokal.

### Kepemilikan dan hasil

- Klaim kandidat menggunakan transaksi singkat: pilih eligible job dengan `FOR UPDATE SKIP LOCKED`, lalu ubah state dan catat attempt sebelum commit.
- Jangan melakukan encoding gambar di dalam transaksi atau sambil menahan row lock.
- Mulai fase multi-worker, setiap claim memiliki token/generation baru, owner, dan lease expiry. Update progress, renewal, dan finalisasi memeriksa token serta state/lease yang sah.
- Worker yang kehilangan kepemilikan tidak boleh memublikasikan hasil, meskipun komputasinya masih berjalan.
- Lease mencegah job selamanya tersangkut; fencing token menolak penulisan worker lama. Keduanya diperlukan.
- Output ditulis pada lokasi unik per attempt. API hanya menyajikan artefak yang dirujuk manifest hasil final setelah conditional commit berhasil. Bersihkan output yatim lewat cleanup terjadwal.
- Eksekusi fisik bisa berulang saat crash. Targetnya at-least-once attempts dengan satu hasil final sah, bukan exactly-once execution.
- Kegagalan filesystem/DB tidak atomik bersama: gunakan staging, metadata, final pointer, dan cleanup yang aman. Input yang sudah dirujuk job aktif tidak boleh terhapus oleh cleanup.

### Scheduling dan prioritas

- One-off job tersedia pada `available_at`; worker tidak menjanjikan eksekusi tepat detik itu.
- Prioritas dipilih saat claim: high, normal, low; di dalamnya urut waktu eligible dan ID sebagai tie-breaker. Tidak ada preemption; dokumentasikan risiko starvation prioritas rendah.
- Recurring schedule memakai interval tetap dan anchor UTC. Unikkan `(schedule_id, scheduled_for)` agar dua scheduler tidak membuat occurrence ganda.
- Buat occurrence dan majukan cursor jadwal dalam satu transaksi.
- Kebijakan missed run v1: coalesce ke maksimal satu occurrence untuk slot terakhir yang jatuh tempo, lewati slot lama; kemudian majukan ke slot masa depan sesuai anchor.
- Kebijakan overlap v1: jika occurrence sebelumnya masih nonterminal, slot baru dilewati dan dicatat. Job manual tidak memakai larangan overlap ini.
- Snapshot input dan pengaturan pada occurrence; perubahan jadwal tidak mengubah job yang telah dibuat. Pertahankan input yang dirujuk jadwal aktif.

## 6. Peta fase belajar

Fase adalah urutan kemampuan, bukan tenggat. Sebagian sesi cukup 45–90 menit. Anggaran total kasar 100–160 jam; waktu belajar Rust dapat menambah durasi. Semua fase dimulai dengan status belum dikerjakan.

| Fase | Hasil yang terlihat | Fokus belajar |
|---|---|---|
| 0 | Lingkungan siap dan roadmap dipahami | Cargo, Git, konsep job |
| 1 | API membuat dan membaca job di memori | Rust dasar, HTTP, enum, Result |
| 2 | Job persisten dijalankan satu worker | SQL, transaksi, async loop |
| 3 | Dashboard membuat dan memantau job | React, API client, polling |
| 4 | Upload gambar → proses → unduh | File I/O, CPU task, resource limit |
| 5 | Retry, cancel, dan histori attempt | State machine dan failure handling |
| 6 | Jadwal sekali jalan dan berulang | Waktu, occurrence, idempotency |
| 7 | Dua worker dan pemulihan crash | Lease, heartbeat, fencing |
| 8 | Aplikasi aman untuk demo terbatas | Login, security, operasional |
| 9 | Portofolio siap ditampilkan | CI, benchmark, dokumentasi |

### Fase 0 — Fondasi dan orientasi

**Prasyarat:** tidak ada; periksa pemahaman Rust dari percakapan, jangan menganggap pengguna sudah mahir.

**Belajar:** Cargo workspace, binary/library, ownership dasar, perbedaan API dan worker, Git dan environment.

**Sublangkah:**

- [ ] 0.1 Periksa Rust/Cargo, Node, Git dan Docker; catat versi dan kekurangan tanpa menginstal diam-diam di luar izin lingkungan.
- [ ] 0.2 Buat repository/workspace minimal, `.gitignore`, contoh environment tanpa secret, README awal, dan checkpoint.
- [ ] 0.3 Tulis definisi job, attempt, queue dan scheduler dalam `docs/glossary.md`.
- [ ] 0.4 Buat binary Rust minimal dan unit test kecil untuk validasi nama job.

**Verifikasi:** build dan test workspace minimal berjalan. Pengguna dapat menunjuk entry point program.

**Latihan:** tambahkan satu aturan validasi nama job dan test-nya.

**Pertanyaan:** apa perbedaan binary dan library? Mengapa job berbeda dari attempt?

**Gate:** command setup tercatat, kode minimal berjalan, progress ditulis. Belum perlu database atau dashboard.

### Fase 1 — Job API sederhana

**Prasyarat:** fase 0. **Belajar:** struct, enum, `Result`, serde, HTTP status, shared state dan async handler.

**Sublangkah:**

- [ ] 1.1 Definisikan `JobId`, `JobStatus`, request/response dan validasi domain.
- [ ] 1.2 Buat health endpoint, `POST /api/v1/jobs`, `GET /api/v1/jobs`, dan detail job.
- [ ] 1.3 Gunakan penyimpanan memori sementara dengan sinkronisasi jelas; jangan memegang guard saat `.await`.
- [ ] 1.4 Buat respons error konsisten dan contoh curl yang benar-benar dicoba.

**Verifikasi:** request valid menghasilkan job queued, ID salah memberi 404, payload invalid ditolak. Restart menghapus data dan keterbatasan itu tertulis.

**Latihan:** tambahkan field deskripsi opsional.

**Pertanyaan:** mengapa validasi juga diperlukan di backend? Mengapa memory store tidak durable?

**Gate:** API create/list/detail bekerja; job belum dieksekusi, tidak ada klaim persistence.

### Fase 2 — Database dan satu worker

**Prasyarat:** fase 1. **Belajar:** SQL, migration, connection pool, transaksi dan task asynchronous.

**Sublangkah:**

- [ ] 2.1 Jalankan PostgreSQL lokal, migrasikan `jobs` dan `job_attempts`, ganti memory store dengan SQLx.
- [ ] 2.2 Buat worker executable; satu worker dan satu job bersamaan dahulu. Klaim tetap memakai transaksi atomik.
- [ ] 2.3 Tambahkan handler simulasi lokal `demo_delay` dengan async timer; bukan sleep blocking. Buat job berjalan lalu selesai dengan hasil sederhana.
- [ ] 2.4 Catat progress dan attempt serta tangani shutdown normal.

**Verifikasi:** job bertahan setelah API restart; dua job diproses sesuai urutan eligibility. Kegagalan DB tidak menghasilkan klaim sukses palsu.

**Latihan:** ubah durasi job demo dan tampilkan durasi aktual.

**Pertanyaan:** mengapa row lock tidak boleh ditahan sepanjang pekerjaan? Apa yang terjadi jika worker mati setelah claim?

**Gate:** API dan worker terpisah bekerja lewat PostgreSQL. Recovery crash belum selesai; job tersangkut pada fase ini dicatat dan hanya direset lewat alat khusus database demo, tidak disembunyikan.

### Fase 3 — Dashboard pertama

**Prasyarat:** fase 2. **Belajar:** React component, TypeScript, form, API state dan polling.

**Sublangkah:**

- [ ] 3.1 Buat halaman Jobs, form create dan Job Detail.
- [ ] 3.2 Hubungkan ke API nyata, polling sekitar dua detik, hentikan polling saat unmount, hindari request overlap.
- [ ] 3.3 Tampilkan queued/running/succeeded/failed, progress, dan waktu.
- [ ] 3.4 Tambahkan loading, empty dan error state serta kontrol keyboard/label form.

**Verifikasi:** pengguna membuat demo job dari browser dan melihat selesai tanpa refresh manual. API mati menghasilkan pesan yang jelas, bukan daftar kosong menyesatkan.

**Latihan:** tambahkan filter status.

**Pertanyaan:** apa perbedaan state loading dan daftar kosong? Mengapa polling harus dibersihkan?

**Gate:** ada alur browser → API → worker → dashboard. Tidak perlu WebSocket atau desain visual kompleks.

### Fase 4 — Job gambar nyata

**Prasyarat:** fase 3. **Belajar:** multipart upload, filesystem, batas resource, CPU-bound vs I/O-bound.

**Sublangkah:**

- [ ] 4.1 Tambahkan upload dan metadata input. Batas awal: 10 file/job, 5 MiB/file, total 25 MiB, maksimum 12 megapixel/gambar; periksa header/dimensi sebelum alokasi decode besar.
- [ ] 4.2 Validasi format nyata JPEG/PNG, bukan ekstensi saja; tolak animasi dan format yang belum didukung. Gunakan storage key buatan server, bukan path dari nama upload.
- [ ] 4.3 Implementasikan resize tanpa memperbesar gambar, JPEG quality tervalidasi, dan latar untuk transparansi.
- [ ] 4.4 Jalankan komputasi lewat blocking boundary yang dibatasi semaphore; simpan output per attempt dan publikasikan manifest setelah sukses.
- [ ] 4.5 Tambahkan download dan UI ukuran input/output serta progres item.

**Verifikasi:** gambar output dapat dibuka dan dimensinya sesuai; input rusak ditolak/gagal dengan pesan aman; file oversized tidak diproses. Job tidak mengklaim ukuran pasti mengecil.

**Latihan:** tambahkan pilihan maksimum lebar gambar pada UI.

**Pertanyaan:** mengapa encoding tidak dijalankan langsung pada async executor? Mengapa nama file klien bukan path penyimpanan?

**Gate:** upload → worker → unduh benar-benar bekerja. Satu job gagal bila satu item gagal; jangan menyajikan hasil parsial sebagai sukses. Progres berbasis item per attempt, bukan estimasi persentase CPU.

### Fase 5 — Retry, cancel, dan attempt history

**Prasyarat:** fase 4. **Belajar:** state machine, error classification, backoff, cancellation kooperatif.

**Sublangkah:**

- [ ] 5.1 Bedakan transient error dan permanent error; file invalid tidak diretry terus-menerus.
- [ ] 5.2 Tambahkan retry dengan backoff, `available_at`, batas `max_attempts`, dan histori error yang aman.
- [ ] 5.3 Tambahkan cancel pending job dan cancel request pada running job.
- [ ] 5.4 Worker memeriksa cancel di batas antaritem dan sebelum publikasi. Hasil tidak dipublikasikan bila cancel menang pada conditional commit.
- [ ] 5.5 Tambahkan manual retry sebagai job baru yang terhubung ke job lama; pertahankan histori.

**Verifikasi:** kegagalan sementara yang diinjeksikan secara deterministik pulih; kegagalan permanen berhenti; cancel/completion race menghasilkan tepat satu terminal state yang valid.

**Latihan:** tampilkan jumlah attempt dan next retry time pada detail job.

**Pertanyaan:** mengapa retry memiliki batas? Mengapa cancel request belum berarti pekerjaan sudah berhenti?

**Gate:** seluruh transisi utama diuji. Jangan mengklaim timeout/abort async otomatis menghentikan `spawn_blocking` yang sudah berjalan. Cancel menunggu batas aman; hard kill memerlukan isolasi proses pada pengembangan berikutnya.

### Fase 6 — Scheduling dan prioritas

**Prasyarat:** fase 5. **Belajar:** UTC, clock injection, recurrence, idempotency dan transaksi scheduler.

**Sublangkah:**

- [ ] 6.1 Tambahkan waktu one-off; job future terlihat Scheduled dan tidak diambil sebelum jatuh tempo.
- [ ] 6.2 Tambahkan high/normal/low pada claim; dokumentasikan tidak ada preemption.
- [ ] 6.3 Buat `schedules`, template input/settings dan occurrence uniqueness.
- [ ] 6.4 Implementasikan interval tetap sesuai kontrak coalesce/no-overlap di bagian 5; dukung enable/disable dan edit untuk future occurrences.
- [ ] 6.5 Buat halaman Schedules serta histori slot yang dijalankan/dilewati.

**Verifikasi:** clock terkontrol menguji batas waktu; restart scheduler tidak menggandakan occurrence; dua scheduler concurrent tetap membuat satu job per slot. Jadwal tidak memakai input yang sudah dibersihkan.

**Latihan:** buat jadwal lokal dengan interval satu menit dan amati dua occurrence.

**Pertanyaan:** bagaimana scheduled time berbeda dari start time? Apa kebijakan jika server mati satu jam?

**Gate:** one-off, interval, prioritas, missed run dan overlap memiliki perilaku serta test eksplisit. Cron dan DST tetap ditunda.

### Fase 7 — Banyak worker dan pemulihan crash

**Prasyarat:** fase 6. **Belajar:** race condition, lease, heartbeat, fencing token dan duplicate execution.

**Sublangkah:**

- [ ] 7.1 Tambahkan worker registration/heartbeat, lease expiry, claim token dan conditional updates.
- [ ] 7.2 Implementasikan renewal dan reclaimer untuk expired attempts, tunduk pada retry limit serta cancel state.
- [ ] 7.3 Tambahkan concurrency limit per worker; hentikan claim saat shutdown dan drain secara terbatas.
- [ ] 7.4 Jalankan dua worker pada satu host yang mengakses volume input/output yang sama; tampilkan worker dan attempt ownership di UI.
- [ ] 7.5 Uji worker lama hidup lagi setelah job diambil worker baru; tolak progress/finalisasi dengan token lama.

**Verifikasi:** kill worker saat proses, tunggu lease habis, worker lain menyelesaikan job; hanya manifest sah yang dapat diunduh. Unikkan artifact per attempt agar worker lama tidak menimpa output baru. Uji juga crash setelah file ditulis tetapi sebelum commit.

**Latihan:** catat timeline recovery dari kill sampai completion dan jelaskan sumber waktu tunggunya.

**Pertanyaan:** mengapa lease saja tidak cukup? Bisakah komputasi berjalan dua kali meskipun final result hanya satu?

**Gate:** klaim concurrent, stale worker, cleanup artefak dan limit retry lulus pengujian. Belum mendukung banyak host dengan filesystem lokal berbeda.

### Fase 8 — Keamanan dan operasional demo

**Prasyarat:** fase 7. Sejak fase awal bind ke localhost; jangan mengekspos instance sebelum kontrol akses selesai.

**Belajar:** session, authorization, CSRF, resource budget dan retensi.

**Sublangkah:**

- [ ] 8.1 Tambahkan seed akun pemilik, password hash dengan library tepercaya, session expiry, login/logout dan cookie aman.
- [ ] 8.2 Lindungi create/cancel/retry/upload/download, validasi ownership, CSRF pada mutasi berbasis cookie, serta rate limit login/upload.
- [ ] 8.3 Tetapkan budget disk dan resource; cleanup output yatim/expired menjaga input job aktif dan schedule aktif.
- [ ] 8.4 Tambahkan liveness/readiness, log terstruktur dengan job/attempt/worker ID, queue depth, wait time, attempt duration dan failures.
- [ ] 8.5 Coba backup database beserta file terkait dan restore konsisten ke lingkungan baru; dokumentasikan prosedur quiesce bila diperlukan.

**Verifikasi:** pengguna tanpa session tidak bisa membaca hasil; nama file traversal tidak keluar storage root; cleanup tidak menghapus file aktif; secret tidak masuk log/commit.

**Latihan:** buat checklist akses endpoint dan buktikan satu request unauthorized ditolak.

**Pertanyaan:** mengapa backup database saja belum cukup? Mengapa status worker offline bukan bukti prosesnya mati?

**Gate:** demo terbatas memiliki akses terkontrol, limit resource, dan restore yang pernah dicoba. Tidak ada klaim aman menjalankan kode arbitrer.

### Fase 9 — Verifikasi akhir dan portofolio

**Prasyarat:** fase 8. **Belajar:** reproducibility, CI, benchmarking, ADR dan komunikasi teknis.

**Sublangkah:**

- [ ] 9.1 Tambahkan CI format/lint/test Rust, integration test PostgreSQL, serta typecheck/build frontend.
- [ ] 9.2 Buat Compose untuk API, worker, web dan DB dengan shared volume; uji dari clone bersih.
- [ ] 9.3 Jalankan benchmark dengan dataset berizin milik sendiri/sintetis; laporkan ukuran/resolusi gambar, setting, mesin, concurrency dan hasil.
- [ ] 9.4 Pisahkan throughput API, queue wait, encoding duration, end-to-end latency, memory dan failure rate. Jangan mengarang angka target sebagai hasil.
- [ ] 9.5 Tulis README Inggris, OpenAPI, tiga ADR utama: database queue, lease/fencing, dan scheduling policy.
- [ ] 9.6 Rekam demo 3–5 menit: upload → progres → unduh → retry → schedule → kill/recovery worker.

**Verifikasi:** petunjuk startup dapat direproduksi; failure tests lulus; screenshot memakai data nyata dari demo; keterbatasan diketahui tercatat.

**Latihan:** jelaskan desain dalam dua menit tanpa membaca kode.

**Pertanyaan:** di mana bottleneck hasil pengukuran? Kapan broker atau object storage benar-benar dibutuhkan?

**Gate:** proyek layak ditampilkan dengan bukti, bukan hanya screenshot. Hosting publik opsional; publikasi mengikuti permintaan pengguna dan akses yang tersedia.

## 7. Kontrak data dan API awal

Tambahkan field saat fasenya tiba, bukan seluruh schema pada hari pertama.

| Entitas | Data inti |
|---|---|
| jobs | ID, type, state, settings snapshot, priority, available_at, attempt limit, cancel request, retry link, final manifest |
| job_attempts | Job ID, attempt number, token, worker, start/end, lease expiry, progress, failure classification |
| artifacts | Storage key, type input/output, size, media type, checksum, association job/attempt |
| workers | ID, last heartbeat, concurrency configuration |
| schedules | Interval, anchor, next slot, input/settings, enabled |
| schedule_occurrences | Schedule ID, slot timestamp, job ID atau alasan skipped |
| users / sessions | Akun pemilik dan session yang dapat dicabut |

Endpoint target: `/api/v1/jobs`, `/jobs/:id`, `/jobs/:id/cancel`, `/jobs/:id/retry`, `/jobs/:id/attempts`, `/uploads`, `/artifacts/:id/download`, `/schedules`, `/workers`, `/session`; seluruh resource tersebut berada di prefix `/api/v1`. Gunakan method dan response schema yang didokumentasikan saat endpoint dibuat. Health endpoints terpisah.

Pada fase yang relevan, tambahkan idempotency key untuk create job: scope pemilik + key unik, request identik mengembalikan job lama, isi berbeda memberi conflict. Ini mencegah double-submit; berbeda dari deduplikasi attempt atau occurrence.

## 8. Standar coding dan verifikasi

- Domain menggunakan tipe/enum dan `Result`; hindari panic/unwrap pada jalur request/worker. Tests boleh memakai expect dengan konteks.
- SQL selalu parameterized; constraint database melindungi uniqueness dan relasi. Migration yang sudah diterapkan tidak ditulis ulang.
- Gunakan structured logs tanpa secret atau isi file. Task blocking harus memiliki batas concurrency dan ukuran input.
- Jangan menyimpan transaksi terbuka saat proses file atau menunggu network. Jangan menulis unsafe tanpa kebutuhan dan kontrak keselamatan yang jelas.
- Test behavior yang berisiko: transition, boundary waktu, transaksi claim, cancellation race, stale token, path handling. Jangan menambah test yang hanya menyalin implementasi untuk perubahan dokumentasi.
- Unit test domain tanpa DB; integration test memakai PostgreSQL nyata. Mock hanya pada batas yang memang relevan. Gunakan fake clock untuk scheduling, bukan sleep panjang.
- Periksa manifest/README/CI sebelum menjalankan command. Setelah workspace dan lockfile ada, baseline Rust: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`.
- Ikuti script package.json dan lockfile frontend yang nyata; jangan mengarang command. Konfigurasikan SQLx build database atau offline metadata bila memakai checked macros.
- Jalankan targeted tests dahulu dan gate CI yang relevan. Laporkan command beserta hasil, serta bagian yang belum bisa diverifikasi.
- Update `docs/progress.md` sesudah sublangkah implementasi. Pilihan teknis besar dicatat sebagai ADR singkat dengan masalah, keputusan, alternatif dan konsekuensi.

## 9. Contoh instruksi pengguna kepada coding agent

- “Baca AGENT.md. Mulai fase 0, sublangkah 0.1. Jelaskan konsep yang perlu saya pahami.”
- “Bantu saya mengerjakan fase 2.2. Tunjukkan alur claim job dan transaksi.”
- “Review kode saya untuk fase 4 sebelum menambahkan fitur berikutnya.”
- “Lanjutkan dari docs/progress.md dan kerjakan sublangkah berikutnya.”
- “Implementasikan fase 5 sampai acceptance criteria selesai, lalu jelaskan perubahan dan test-nya.”
- “Jelaskan fencing token dengan contoh dari kode TaskHarbor yang sudah ada.”

## 10. Referensi teknis

Referensi berikut mendukung pilihan mekanisme; pembagian fase dan aturan produk adalah rancangan TaskHarbor.

- [Axum](https://docs.rs/axum/latest/axum/): routing HTTP dan integrasi Tokio/Tower.
- [PostgreSQL SELECT dan locking](https://www.postgresql.org/docs/current/sql-select.html): `FOR UPDATE SKIP LOCKED` dapat digunakan untuk menghindari perebutan row antrean; bukan jaminan FIFO global atau solusi seluruh race condition.
- [Tokio spawn_blocking](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html): isolasi pekerjaan blocking, pembatasan concurrency CPU, dan keterbatasan abort setelah pekerjaan mulai.
