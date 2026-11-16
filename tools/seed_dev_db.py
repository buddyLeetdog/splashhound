import sqlite3
import os
import sys

db = os.path.join(os.environ["APPDATA"], "com.splashhound.desktop", "library.db")
root = r"D:\InflaTool\example videos"
conn = sqlite3.connect(db)
conn.execute(
    "INSERT INTO settings(key, value) VALUES('root', ?) "
    "ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    (root,),
)
conn.commit()
print("settings:", conn.execute("SELECT key, value FROM settings").fetchall())
print("videos:", conn.execute("SELECT COUNT(*) FROM videos").fetchone()[0])
print("reviewed:", conn.execute("SELECT COUNT(*) FROM videos WHERE reviewed = 1").fetchone()[0])
conn.close()
