import Database from 'tauri-plugin-sql-api';

let database;
export function openHistory() {
    database ??= Database.load('sqlite:history.db').then(async db => {
        await db.execute('CREATE TABLE IF NOT EXISTS history(id INTEGER PRIMARY KEY AUTOINCREMENT, text TEXT NOT NULL,source TEXT NOT NULL,target TEXT NOT NULL,service TEXT NOT NULL, result TEXT NOT NULL,timestamp INTEGER NOT NULL)');
        return db;
    }).catch(error => { database = null; throw error; });
    return database;
}

export async function addHistory(text, source, target, service, result) {
    const db = await openHistory();
    // The SQL plugin shares the connection with all engine cards and History.
    // Closing it after one result invalidates concurrent inserts and reads.
    await db.execute('INSERT into history (text, source, target, service, result, timestamp) VALUES ($1, $2, $3, $4, $5, $6)', [text, source, target, service, result, Date.now()]);
}
