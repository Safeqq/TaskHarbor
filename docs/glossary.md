# Glosarium TaskHarbor

## Job

Job adalah catatan pekerjaan yang diminta pengguna, misalnya "ubah ukuran tiga gambar". Job menyimpan maksud dan status keseluruhan pekerjaan. Dalam fase berikutnya, sebuah job dapat tetap ada walaupun proses yang mengerjakannya sempat gagal.

## Attempt

Attempt adalah satu usaha eksekusi untuk sebuah job. Satu job dapat memiliki beberapa attempt karena kegagalan sementara atau pemulihan worker. Memisahkan keduanya menjaga identitas job tetap sama sekaligus membuat setiap usaha, waktu mulai, hasil, dan alasan gagal dapat dicatat sendiri.

## Queue

Queue atau antrean adalah kumpulan job yang menunggu giliran diproses. Worker mengambil job yang sudah layak dijalankan sesuai aturan waktu dan prioritas. TaskHarbor memakai PostgreSQL sebagai antrean durable agar data tidak hilang saat proses aplikasi dimulai ulang.

## Scheduler

Scheduler adalah komponen yang menentukan kapan pekerjaan terjadwal menjadi siap dijalankan. Untuk jadwal berulang, scheduler membuat occurrence job berdasarkan interval dan anchor waktu. Scheduler tidak mengerjakan isi job; worker tetap menjalankan pekerjaan tersebut.

## API

API adalah program yang menerima permintaan dari dashboard atau klien lain, memvalidasi input, lalu membaca atau mengubah data aplikasi. API tidak akan melakukan pemrosesan gambar yang lama di dalam request.

## Worker

Worker adalah program terpisah yang mengambil job dari queue, menjalankan pekerjaan, dan menyimpan progres atau hasilnya. Pemisahan API dan worker membuat request pengguna tetap singkat walaupun pemrosesan memerlukan waktu lebih lama.

## Artifact

Artifact adalah file yang terkait dengan job. Input artifact menyimpan metadata gambar sumber, sedangkan output artifact menunjuk JPEG hasil suatu attempt. Nama file dari klien hanya menjadi label; path penyimpanan dibuat server agar tidak dapat mengarahkan penulisan ke lokasi sembarang.

## Manifest output

Manifest output adalah kumpulan catatan output yang dinyatakan siap diunduh. TaskHarbor memasukkan seluruh output dan mengubah job menjadi `succeeded` dalam satu transaksi, sehingga pengguna tidak melihat hasil parsial sebagai keberhasilan penuh.

## Blocking task

Blocking task adalah pekerjaan sinkron yang memakai thread sampai selesai, seperti decode, resize, dan encode gambar. Worker menjalankannya melalui `spawn_blocking` dengan semaphore agar pekerjaan CPU tidak menahan thread async dan jumlah pekerjaan berat tetap dibatasi.

## Transient error

Transient error adalah kegagalan sementara yang mungkin pulih jika dicoba lagi, misalnya file input untuk sementara tidak dapat dibaca. TaskHarbor menyimpan attempt yang gagal, menunggu backoff, lalu membuat attempt baru selama batas usaha belum habis.

## Permanent error

Permanent error adalah kegagalan yang tidak akan membaik hanya dengan mengulang input yang sama, misalnya data gambar rusak atau pengaturan tidak valid. Job langsung menjadi `failed` agar tidak menghabiskan resource dengan retry tanpa manfaat.

## Backoff

Backoff adalah jeda sebelum automatic retry. TaskHarbor memakai jeda eksponensial 1, 2, 4 detik, dan seterusnya dengan batas 60 detik. `available_at` menyimpan waktu paling awal job boleh diklaim lagi.

## Cooperative cancellation

Cooperative cancellation berarti worker berhenti ketika mencapai batas aman yang sudah ditentukan. Job berjalan lebih dahulu menjadi `cancel_requested`; worker memeriksanya di antara item dan sebelum publikasi hasil, membersihkan output attempt, lalu mengubahnya menjadi `cancelled`. Pekerjaan `spawn_blocking` yang sudah dimulai tidak dapat dihentikan paksa oleh mekanisme ini.

## Manual retry

Manual retry membuat job `queued` baru dengan `retry_of_job_id` yang menunjuk job `failed` atau `cancelled`. Job lama tetap terminal beserta seluruh histori attempt-nya, sehingga audit tidak ditulis ulang.

## UTC anchor

UTC anchor adalah titik acuan tetap untuk menghitung seluruh slot recurring schedule. Jika anchor berada pada 08:00 UTC dengan interval satu jam, slot berikutnya tetap 09:00, 10:00, dan seterusnya; waktu mulai worker yang terlambat tidak menggeser pola tersebut.

## Occurrence

Occurrence adalah catatan satu slot schedule yang sudah ditangani. Hasilnya dapat berupa job baru atau `skipped_overlap` ketika occurrence sebelumnya masih nonterminal. Pasangan schedule dan `scheduled_for` unik agar restart atau dua scheduler tidak membuat slot yang sama dua kali.

## Coalescing

Coalescing adalah kebijakan merangkum banyak slot yang terlewat menjadi maksimal satu occurrence untuk slot terakhir yang sudah jatuh tempo. TaskHarbor mencatat jumlah slot lama yang dilewati dan memajukan cursor ke slot berikutnya sesuai anchor.

## Priority dan preemption

Priority menentukan job eligible mana yang diklaim lebih dahulu: `high`, `normal`, lalu `low`. Di dalam priority yang sama, urutannya memakai `available_at` lalu ID. TaskHarbor belum melakukan preemption, sehingga job yang sudah berjalan tidak dihentikan ketika job priority lebih tinggi datang.

## Heartbeat

Heartbeat adalah pembaruan berkala dari worker ke PostgreSQL untuk menunjukkan bahwa proses itu masih berkomunikasi. Dashboard menyebut worker `offline` setelah batas heartbeat terlewati, tetapi status itu tetap sebuah inferensi dan bukan pemeriksaan langsung terhadap proses sistem operasi.

## Lease

Lease adalah hak menjalankan attempt sampai waktu tertentu. Worker memperpanjang lease selama pekerjaan berlangsung. Jika lease kedaluwarsa, reclaimer boleh menutup attempt lama dan membuat job tersedia untuk retry.

## Fencing token

Fencing token adalah nilai acak unik untuk satu claim. Setiap penulisan progress atau hasil harus membawa worker ID dan token yang masih cocok, sehingga worker lama yang hidup kembali tidak dapat menimpa hasil attempt pengganti.

## Reclaimer

Reclaimer adalah proses yang mencari attempt `running` dengan lease kedaluwarsa. Ia mengunci satu attempt secara transaksional, lalu menyelesaikan cancel, menjadwalkan retry, atau menggagalkan job ketika batas attempt habis.

## At-least-once execution

At-least-once execution berarti sebuah job dapat menjalankan komputasi fisik lebih dari sekali ketika terjadi crash dan recovery. TaskHarbor membatasi dampaknya dengan output per attempt dan fencing token, sehingga hanya attempt aktif yang dapat mempublikasikan manifest final.

## Graceful drain

Graceful drain adalah tahap shutdown saat worker berhenti mengambil job baru tetapi memberi task aktif waktu terbatas untuk selesai. Setelah grace period habis, claim akan dibiarkan kedaluwarsa agar worker lain dapat memulihkannya.

## Session

Session adalah bukti login yang disimpan server dan memiliki waktu kedaluwarsa. Browser hanya menerima token acak melalui cookie; PostgreSQL menyimpan hash token tersebut sehingga isi database tidak langsung menjadi kredensial aktif. Logout mencabut session sebelum masa berlakunya habis.

## Authentication dan authorization

Authentication menjawab siapa pengguna yang mengirim request, sedangkan authorization menentukan resource dan tindakan yang boleh diaksesnya. TaskHarbor mengautentikasi satu akun pemilik melalui session lalu memfilter job, schedule, dan artifact berdasarkan `owner_user_id`.

## CSRF

Cross-Site Request Forgery adalah upaya membuat browser yang sudah login mengirim mutasi tanpa kehendak pengguna. Cookie dapat ikut terkirim otomatis, sehingga TaskHarbor juga mewajibkan token CSRF pada header untuk `POST`, `PUT`, dan `DELETE`. Situs lain tidak dapat membaca token tersebut dari respons same-origin.

## Rate limit

Rate limit membatasi jumlah request dalam suatu jendela waktu. TaskHarbor membatasi percobaan login dan upload pada setiap proses API untuk mengurangi brute force dan konsumsi resource berulang. Batas ini cocok untuk demo satu proses; deployment banyak instance memerlukan limiter bersama.

## Idempotency key

Idempotency key adalah label request create job yang unik dalam scope pemilik. Key dan fingerprint data yang sama mengembalikan job lama, sedangkan key yang sama dengan data berbeda menghasilkan conflict. Ini mencegah double-submit dan berbeda dari fencing attempt maupun uniqueness occurrence schedule.

## Retention dan orphan grace

Retention menentukan berapa lama output terminal dipertahankan sebelum referensinya kedaluwarsa. Orphan adalah file yang tidak dirujuk database. Grace period memberi waktu bagi commit database yang terlambat atau proses aktif sebelum orphan boleh dihapus; input yang masih direferensikan dan prefix attempt aktif selalu dilindungi.

## Liveness dan readiness

Liveness menjawab apakah proses API masih berjalan dan tidak bergantung pada database. Readiness menjawab apakah API siap melayani request dengan memeriksa koneksi PostgreSQL dan kemampuan menulis storage. Pemisahan ini mencegah gangguan dependency disalahartikan sebagai proses yang mati.

## Quiesced backup

Quiesced berarti API dan seluruh worker dihentikan sementara agar database dan filesystem tidak berubah selama backup. TaskHarbor perlu menyalin keduanya dari titik yang konsisten karena metadata artifact berada di PostgreSQL sementara byte file berada di shared storage.
