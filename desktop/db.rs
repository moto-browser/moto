/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::path::Path;

use rusqlite::{Connection, Error};

use super::minibrowser::Bookmark;

pub fn create_bookmarks_db(config_dir: &str) -> Result<(), Error> {
    let path = Path::new(&config_dir).join("bookmarks.sqlite");
    let conn = Connection::open(path)?;
    conn.execute(
        "CREATE TABLE bookmarks (id INTEGER PRIMARY KEY, url TEXT, title TEXT)",
        [],
    )?;
    Ok(())
}

pub fn get_bookmarks(config_dir: &str) -> Vec<Bookmark> {
    let path = Path::new(&config_dir).join("bookmarks.sqlite");
    let conn = Connection::open(path).expect("Failed to open connection to DB!");
    let mut stmt = conn.prepare("SELECT * FROM bookmarks").unwrap();
    let rows = stmt.query([]).unwrap();
    rows.mapped(|row| {
        let url: String = row.get(1).unwrap();
        let title: String = row.get(2).unwrap();
        Ok(Bookmark { url, title })
    })
    .map(|bookmark| bookmark.unwrap())
    .collect()
}

pub fn add_bookmark(config_dir: &str, url: &str, title: &str) -> Result<(), Error> {
    let path = Path::new(&config_dir).join("bookmarks.sqlite");
    let conn = Connection::open(path).expect("Failed to open connection to DB!");
    conn.execute(
        "INSERT INTO bookmarks (url, title) VALUES (?1, ?2)",
        [&url, &title],
    )?;
    Ok(())
}

pub fn remove_bookmark(config_dir: &str, url: &str) -> Result<(), Error> {
    let path = Path::new(&config_dir).join("bookmarks.sqlite");
    let conn = Connection::open(path).expect("Failed to open connection to DB!");
    conn.execute("DELETE FROM bookmarks WHERE url = ?1", [&url])?;
    Ok(())
}
