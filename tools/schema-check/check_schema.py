"""Şema duman testi: migrasyonu gerçek SQLite + sqlite-vec üzerinde çalıştırır,
kısıtları, tetikleyicileri ve indeks planlarını kontrol eder.

Kullanım: pip install sqlite-vec && python3 tools/schema-check/check_schema.py crates/loupe-db/migrations
(Klasördeki NNNN_*.sql dosyaları sırayla, her biri tek transaction'da uygulanır.)
"""
import sqlite3, sqlite_vec, sys, struct, os
db = sqlite3.connect(":memory:", isolation_level=None)
db.enable_load_extension(True); sqlite_vec.load(db)
for p in ["foreign_keys=ON","journal_mode=WAL","synchronous=NORMAL","mmap_size=268435456"]:
    db.execute("PRAGMA "+p)
import glob
migrations = sorted(glob.glob(os.path.join(sys.argv[1], "[0-9][0-9][0-9][0-9]_*.sql")))
assert migrations, "migrasyon bulunamadı"
for m in migrations:
    db.executescript("BEGIN;\n" + open(m, encoding="utf-8").read() + "\nCOMMIT;")
    print("applied", os.path.basename(m))
print("sqlite", sqlite3.sqlite_version, "vec", db.execute("select vec_version()").fetchone()[0])
print("user_version", db.execute("PRAGMA user_version").fetchone()[0])
print("fk violations", db.execute("PRAGMA foreign_key_check").fetchall())
print("integrity", db.execute("PRAGMA integrity_check").fetchone()[0])
tables = [r[0] for r in db.execute("select name from sqlite_master where type='table' and name not like 'vec_%' and name not like 'sqlite_%' order by name")]
print("tables", tables)
T=1761390000000
db.execute("insert into libraries(uuid,name,kind,volume_uuid,rel_root,created_at,updated_at) values(randomblob(16),'Ana','removable','A1B2-C3D4','Fotograflar',?,?)",(T,T))
def media(i, ph, h=None):
    db.execute("""insert into media_files(library_id,rel_path,original_filename,placement,file_type,mime,size_bytes,mtime_ns,blake3,phash,taken_at_utc,taken_at_source,last_verified_at,imported_at,updated_at)
     values(1,?,?, 'copy','jpeg','image/jpeg',100,1,?,?,?,'exif',?,?,?)""",(f"2026/10/24/IMG_{i}.JPG",f"IMG_{i}.JPG",h or os.urandom(32),ph,T+i,T,T,T))
media(1, -0x0123456789ABCDEF); media(2, -0x0123456789ABCDEE)
dup = db.execute("select blake3 from media_files where id=1").fetchone()[0]
try: media(3, 5, dup); print("FAIL dup allowed")
except sqlite3.IntegrityError as e: print("dup blocked:", e)
try:
    db.execute("insert into libraries(uuid,name,kind,rel_root,created_at,updated_at) values(randomblob(16),'x','removable','',1,1)"); print("FAIL")
except sqlite3.IntegrityError as e: print("removable w/o uuid blocked:", e)
# phash band query uses index
q="select id from media_files where ((phash>>48)&65535)=((?>>48)&65535) and phash is not null"
print(db.execute("explain query plan "+q,(-0x0123456789ABCDEF,)).fetchall())
db.create_function("hamming",2,lambda a,b: bin((a^b)&0xFFFFFFFFFFFFFFFF).count("1"),deterministic=True)
near = db.execute("""select a.id, b.id, hamming(a.phash, b.phash) from media_files a join media_files b
  on a.id < b.id and (  (a.phash & 65535) = (b.phash & 65535)
                     or ((a.phash >> 16) & 65535) = ((b.phash >> 16) & 65535)
                     or ((a.phash >> 32) & 65535) = ((b.phash >> 32) & 65535)
                     or ((a.phash >> 48) & 65535) = ((b.phash >> 48) & 65535))
  where hamming(a.phash, b.phash) <= 8""").fetchall()
assert near == [(1, 2, 2)], near
print("near-dup pairs", near)
# models, clusters, faces, vec
db.execute("insert into ml_models(task,name,version,dim,file_blake3,created_at) values('face_embed','arcface_r100','1',512,randomblob(32),?)",(T,))
db.execute("insert into persons(name,created_at,updated_at) values('Oğuz',?,?)",(T,T))
db.execute("insert into face_clusters(person_id,model_id,created_at,updated_at) values(1,1,?,?)",(T,T))
db.execute("insert into faces(media_id,cluster_id,bbox_x,bbox_y,bbox_w,bbox_h,det_score,quality,detector_model_id,embedder_model_id,created_at) values(1,1,.1,.1,.2,.2,.9,.8,1,1,?)",(T,))
db.execute("insert into vec_faces(face_id,embedding) values(1,?)",(sqlite_vec.serialize_float32([0.01]*512),))
print("cluster count", db.execute("select face_count from face_clusters").fetchone(), "person", db.execute("select * from v_face_person").fetchall())
print("knn", db.execute("select face_id,distance from vec_faces where embedding match ? and k=1",(sqlite_vec.serialize_float32([0.02]*512),)).fetchall())
db.execute("delete from faces where id=1")
print("after delete: count", db.execute("select face_count from face_clusters").fetchone(), "vec rows", db.execute("select count(*) from vec_faces").fetchone())
# albums / events triggers
db.execute("insert into albums(name,created_at,updated_at) values('Kapadokya',?,?)",(T,T))
db.execute("insert into album_media values(1,1,1.0,?)",(T,)); db.execute("insert into album_media values(1,2,2.0,?)",(T,))
db.execute("insert into events(title,start_utc,end_utc,created_at,updated_at) values('Ekim 2026',?,?,?,?)",(T,T+10,T,T))
db.execute("insert into event_media(event_id,media_id) values(1,1),(1,2)"); db.execute("update event_media set source='user_excluded' where media_id=2")
print("album count", db.execute("select media_count from albums").fetchone(), "event count", db.execute("select media_count from events").fetchone())
# sync status + blake3 change trigger
db.execute("insert into cloud_targets(name,provider,config,secret_ref,created_at,updated_at) values('B2','b2','{}','mp/b2',?,?)",(T,T))
db.execute("insert into sync_status(media_id,target_id,state,local_checksum,remote_key,remote_checksum,remote_checksum_algo,uploaded_at,updated_at) values(1,1,'synced',?,'k','abc','sha1',?,?)",(dup,T,T))
try: db.execute("insert into sync_status(media_id,target_id,state,local_checksum,updated_at) values(2,1,'synced',randomblob(32),1)"); print("FAIL")
except sqlite3.IntegrityError as e: print("synced w/o remote blocked:", e)
db.execute("insert into thumbnails values(1,'grid','webp',256,171,9000,?,0,'decode',?,NULL)",(dup,T))
db.execute("update media_files set blake3=randomblob(32), updated_at=? where id=1",(T+1,))
print("after content change: thumbs", db.execute("select count(*) from thumbnails").fetchone(), "sync", db.execute("select state from sync_status").fetchone())
db.execute("update media_files set rating=4 where id=2"); print("sidecar_dirty", db.execute("select sidecar_dirty from media_files where id=2").fetchone())
try: db.execute("delete from media_files where id=1"); print("FAIL")
except sqlite3.IntegrityError as e: print("delete synced media blocked:", e)
# edits
db.execute("insert into versions(media_id,is_current,created_at,updated_at) values(2,1,?,?)",(T,T))
db.execute("insert into edits(version_id,seq,op,params,engine_version,created_at) values(1,0,'exposure','{\"ev\":0.3}',1,?)",(T,))
print("revision", db.execute("select revision from versions").fetchone())
# timeline plan
print(db.execute("explain query plan select id from media_files where soft_deleted_at is null and taken_at_utc < ? order by taken_at_utc desc, id desc limit 200",(T+5,)).fetchall())

# ── 0002: iş kuyruğu, bulut kuyruğu, LAN ──
db.execute("insert into jobs(kind,media_id,variant,base_priority,run_after,created_at,updated_at) values('thumb',2,'grid',0,?,?,?)",(T,T,T))
try: db.execute("insert into jobs(kind,media_id,variant,base_priority,run_after,created_at,updated_at) values('thumb',2,'grid',5,?,?,?)",(T,T,T)); print("FAIL")
except sqlite3.IntegrityError as e: print("duplicate job blocked:", e)
try: db.execute("insert into jobs(kind,media_id,variant,base_priority,run_after,created_at,updated_at) values('thumb',2,'huge',0,?,?,?)",(T,T,T)); print("FAIL")
except sqlite3.IntegrityError as e: print("bad thumb variant blocked:", e)
try: db.execute("update jobs set state='running' where id=1"); print("FAIL")
except sqlite3.IntegrityError as e: print("running without lease blocked:", e)
# atomik kira alma (RETURNING)
row = db.execute("""update jobs set state='running', lease_owner='1/thumb-0', lease_until=?, attempts=attempts+1, updated_at=?
  where id = (select id from jobs where kind='thumb' and state in ('pending','failed') and run_after <= ?
              order by base_priority, run_after, id limit 1) returning id, attempts""",(T+30000,T,T)).fetchone()
assert row == (1, 1), row; print("lease acquired", row)
print(db.execute("explain query plan select id from jobs where kind='thumb' and state in ('pending','failed') and run_after <= ? order by base_priority, run_after, id limit 1",(T,)).fetchall())
db.execute("insert into jobs(kind,variant,base_priority,run_after,created_at,updated_at) values('cluster_faces','',6,?,?,?)",(T,T,T))
try: db.execute("insert into jobs(kind,variant,base_priority,run_after,created_at,updated_at) values('cluster_faces','',6,?,?,?)",(T,T,T)); print("FAIL")
except sqlite3.IntegrityError as e: print("duplicate global job blocked:", e)
db.execute("insert into sync_queue(target_id,media_id,op,run_after,bytes_total,created_at,updated_at) values(1,2,'upload',?,100,?,?)",(T,T,T))
try: db.execute("update sync_queue set bytes_done=200"); print("FAIL")
except sqlite3.IntegrityError as e: print("bytes_done > total blocked:", e)
db.execute("""insert into devices(name,platform,cert_sha256,spki_sha256,cert_serial,cert_not_after,target_library_id,paired_at)
  values('Pixel 9','android',randomblob(32),randomblob(32),randomblob(16),?,1,?)""",(T+10**11,T))
db.execute("insert into lan_transfers(upload_id,device_id,library_id,client_asset_id,filename,expected_blake3,expected_size,staging_name,client_modified_at,created_at,updated_at,expires_at) values('AAAAAAAAAAAAAAAAAAAAAA',1,1,'ms:1234','PXL_1.jpg',randomblob(32),1000,'x.part',?,?,?,?)",(T,T,T,T+7*86400000))
try: db.execute("insert into lan_transfers(upload_id,device_id,library_id,client_asset_id,filename,expected_blake3,expected_size,staging_name,client_modified_at,created_at,updated_at,expires_at) values('BBBBBBBBBBBBBBBBBBBBBB',1,1,'ms:1234','PXL_1.jpg',randomblob(32),1000,'y.part',?,?,?,?)",(T,T,T,T)); print("FAIL")
except sqlite3.IntegrityError as e: print("second active transfer for same asset blocked:", e)
try: db.execute("update lan_transfers set state='done', offset_bytes=1000"); print("FAIL")
except sqlite3.IntegrityError as e: print("done without result blocked:", e)
try: db.execute("update lan_transfers set offset_bytes=2000"); print("FAIL")
except sqlite3.IntegrityError as e: print("offset > size blocked:", e)
print("settings", db.execute("select count(*) from settings").fetchone()[0])
try: db.execute("insert into settings values('Bad Key','1',0)"); print("FAIL")
except sqlite3.IntegrityError as e: print("bad settings key blocked:", e)
db.execute("delete from media_files where id=2")  # medya silinince işleri ve kuyruğu da gider
print("after media delete: jobs", db.execute("select count(*) from jobs where media_id=2").fetchone()[0], "sync_queue", db.execute("select count(*) from sync_queue").fetchone()[0])
print("user_version", db.execute("PRAGMA user_version").fetchone()[0])
print("fk violations", db.execute("PRAGMA foreign_key_check").fetchall())
print("OK")
