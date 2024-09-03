use ncurses::*;
use std::collections::HashMap;
use std::marker::PhantomData;

use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

use std::time::Duration;

use serde::{Deserialize, Serialize};

pub struct WindowManager {
    windows: HashMap<usize, (WINDOW, WINDOW)>,
}

unsafe impl Send for WindowManager {}
unsafe impl Sync for WindowManager {}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PrintCommand {
    pub window: usize,
    pub message: String,
}
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReadCommand {
    pub window: usize,
    pub prompt: String,
}
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NewWindowCommand {
    pub win_number: usize,
    pub start_y: i32,
    pub win_height: i32,
    pub win_width: i32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum WindowCommand {
    Println(PrintCommand),
    Print(PrintCommand),
    Read(ReadCommand),
    New(NewWindowCommand),
    Init(),
    Shutdown(),
}

pub struct WindowPipe {
    pub tx: Arc<Mutex<mpsc::Sender<WindowCommand>>>,
    pub rx: Arc<Mutex<mpsc::Receiver<WindowCommand>>>,
    pub tx_input: Arc<Mutex<mpsc::Sender<String>>>,
    pub rx_input: Arc<Mutex<mpsc::Receiver<String>>>,
}

impl WindowPipe {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(50);
        let (tx_input, rx_input) = mpsc::channel(50);
        Self {
            tx: Arc::new(Mutex::new(tx)),
            rx: Arc::new(Mutex::new(rx)),
            tx_input: Arc::new(Mutex::new(tx_input)),
            rx_input: Arc::new(Mutex::new(rx_input)),
        }
    }

    pub fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            rx: self.rx.clone(),
            tx_input: self.tx_input.clone(),
            rx_input: self.rx_input.clone(),
        }
    }

    pub async fn read(&self) -> Result<WindowCommand, ()> {
        match self.rx.lock().await.recv().await {
            Some(msg) => Ok(msg),
            None => Err(()),
        }
    }

    pub async fn send(&self, cmd: WindowCommand) {
        let _ = self.tx.lock().await.send(cmd).await;
    }

    pub async fn get_input(&self, window: usize, prompt: &str) -> Result<String, ()> {
        let cmd = ReadCommand {
            window,
            prompt: prompt.to_string(),
        };
        let _ = self.tx.lock().await.send(WindowCommand::Read(cmd)).await;
        let mut rx;
        {
            rx = self.rx_input.lock().await;
        }
        match rx.recv().await {
            Some(msg) => Ok(msg),
            None => Err(()),
        }
    }
}

impl WindowManager {
    pub fn new() -> Self {
        WindowManager {
            windows: HashMap::new(),
        }
    }
    pub fn init(&self) {
        // Initialize ncurses
        initscr();
        cbreak();
        noecho();
        keypad(stdscr(), true); // Enable keypad input
        refresh(); // Refresh the standard screen to ensure it's initialized
    }
    pub fn get_max_yx() -> (i32, i32) {
        let mut max_y = 0;
        let mut max_x = 0;
        getmaxyx(stdscr(), &mut max_y, &mut max_x);
        (max_y, max_x)
    }
    pub fn new_window(
        &mut self,
        window_number: usize,
        win_height: i32,
        win_width: i32,
        start_y: i32,
    ) {
        let win = newwin(win_height, win_width, start_y, 0);
        let subwin = derwin(win, win_height - 1, win_width - 1, 1, 1);
        scrollok(subwin, true); // Enable scrolling for the window
        wrefresh(win); // Refresh the window to apply the box
        self.windows.insert(window_number, (win, subwin));
    }

    // Print a message to a specific window
    pub fn printw(&self, window_number: usize, message: &str) {
        if let Some((win, subwin)) = self.windows.get(&window_number) {
            // Print the message in the window
            wprintw(*subwin, message);
            wrefresh(*subwin); // Refresh the window to display the new content
            let cur_y = getcury(*subwin);
            if cur_y + 1 >= getmaxy(*subwin) {
                // Manually scroll the subwindow if the cursor is at the bottom
                let diff = getmaxy(*subwin) - (cur_y + 1);
                wscrl(*subwin, diff);
                wmove(*subwin, getmaxy(*subwin) - diff, 1); // Move cursor to the start of the new line after scroll
            }
        } else {
            println!(
                "Window number {} does not exist. '{}'",
                window_number, message
            );
        }
    }

    // Make window interactive
    pub fn getch(&self, window_number: usize, prompt: &str) -> Option<String> {
        if let Some((win, subwin)) = self.windows.get(&window_number) {
            wtimeout(*subwin, 1000);
            wrefresh(*subwin);

            // Move the cursor to just inside the box, 1 line down, 1 column in
            if prompt.len() > 0 {
                println!("printing something");
                self.printw(window_number, prompt);
                wrefresh(*subwin);
            }
            let mut input = Vec::<u8>::new();
            nocbreak();
            echo();
            curs_set(CURSOR_VISIBILITY::CURSOR_VISIBLE);

            // Handle user input
            let mut cur_y = 0;
            let mut cur_x = 0;
            getyx(*subwin, &mut cur_y, &mut cur_x);
            let mut ch = wgetch(*subwin);
            while ch != '\n' as i32 {
                if ch == ERR {
                    wmove(*subwin, cur_y, cur_x);
                    // Timeout
                    if input.len() == 0 {
                        return None;
                    }
                } else if ch == KEY_BACKSPACE || ch == 127 {
                    if !input.is_empty() {
                        input.pop();
                        wdelch(*subwin);
                    }
                } else {
                    input.push(ch as u8);
                }
                wrefresh(*subwin);
                ch = wgetch(*subwin);
            }

            // Exit condition (optional)
            return Some(
                std::str::from_utf8(input.as_slice())
                    .unwrap()
                    .to_string()
                    .trim()
                    .to_string(),
            );
        } else {
            None
        }
    }

    // Clean up ncurses
    pub fn cleanup(&self) {
        for (_, (win, subwin)) in &self.windows {
            delwin(*subwin);
            delwin(*win);
        }
        endwin(); // End ncurses mode
    }

    pub async fn serve(&mut self, pipe: WindowPipe) {
        let mut keep_running = true;
        while keep_running {
            match pipe.read().await {
                Ok(command) => match command {
                    WindowCommand::Read(cmd) => match self.getch(cmd.window, &cmd.prompt) {
                        Some(input) => {
                            let _ = pipe.tx_input.lock().await.send(input).await;
                        }
                        None => {
                            let _ = pipe.tx_input.lock().await.send("".to_string()).await;
                        }
                    },
                    WindowCommand::Println(cmd) => {
                        self.printw(cmd.window, &format!("{}\n", cmd.message));
                    }
                    WindowCommand::Print(cmd) => {
                        self.printw(cmd.window, &cmd.message);
                    }
                    WindowCommand::New(cmd) => {
                        self.new_window(cmd.win_number, cmd.win_height, cmd.win_width, cmd.start_y);
                    }
                    WindowCommand::Init() => {
                        self.init();
                    }
                    WindowCommand::Shutdown() => {
                        keep_running = false;
                    }
                    _ => {}
                },
                Err(()) => {}
            }
        }
        self.cleanup();
    }
}
