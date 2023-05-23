use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::process;
use std::process::Command;
use std::thread;
use std::time::Duration;

#[cfg(unix)]
use libc::{
    c_ushort, ioctl, STDOUT_FILENO, TIOCGWINSZ, TCSANOW, termios,
    VMIN, VTIME, ICANON, ECHO, IXON, IXOFF,
};

/* Constants and Enums */

const TAB_SPACES: usize = 4;

#[derive(Debug, PartialEq)]
enum Key {
    Backspace,
    Newline,
    Escape,
    CtrlS,
    CtrlQ,
    Char(char),
    UpArrow,
    DownArrow,
    LeftArrow,
    RightArrow,
}


/* Objects */

struct EditorState {
    cursor_x: usize,
    cursor_y: usize,
    current_column: usize,
    current_row: usize,
    file: Vec<Vec<u8>>,
    filename: String,
    menu_info: String,
    rendered_x: usize,
    screen_buffer: String,
    terminal_cols: usize,
    terminal_rows: usize,
    user_input: std::io::StdinLock<'static>,
}


/* Setup */

fn setup(editor_state: &mut EditorState) -> termios {
    // New screen buffer
    print!("\x1B[?1049h");
    clear_screen();

    let stdin_fd = editor_state.user_input.as_raw_fd();

    // Disable terminal buffering
    let mut term_cmd = Command::new("stty");
    term_cmd.arg("-icanon");
    let _ = term_cmd.status();

    let default_termios = default_termios();

    // Put the terminal into raw mode
    setup_terminal(stdin_fd, &default_termios);

    return default_termios
}

fn default_termios() -> termios {
    #[cfg(unix)]
    unsafe {
        let mut termios = std::mem::zeroed();
        libc::tcgetattr(libc::STDIN_FILENO, &mut termios);
        
        return termios
    }
}

fn setup_terminal(fd: RawFd, default_termios: &termios) {
    #[cfg(unix)]
    unsafe {
        let mut termios = default_termios.clone();

        // Disable software flow control (IXON/IXOFF)
        termios.c_iflag &= !(IXON | IXOFF);

        // Set raw mode with minimal input processing (ICANON, ECHO)
        termios.c_lflag &= !(ICANON | ECHO);

        // Set read timeout to return immediately
        termios.c_cc[VMIN] = 0;
        termios.c_cc[VTIME] = 0;

        // Apply the modified terminal attributes
        libc::tcsetattr(fd, TCSANOW, &termios);
    }
}


/* Cleanup */

fn cleanup(termios: &termios) {
    let stdin = io::stdin();
    let stdin_fd = stdin.as_raw_fd();
    cleanup_terminal(stdin_fd, termios);

    // Restore the terminal settings
    let mut cmd = Command::new("stty");
    cmd.args(&["echo", "icanon"]);

    let _ = cmd.status();

    // Main screen buffer
    print!("\x1B[?1049l");
}

fn cleanup_terminal(fd: RawFd, termios: &termios) {
    #[cfg(unix)]
    unsafe {
        // Restore the terminal attributes
        libc::tcsetattr(fd, TCSANOW, termios);
    }
}


/* Input Handling */

enum KeyAction {
    Continue,
    Exit,
}

fn process_input(input: &mut dyn Read) -> io::Result<Option<Key>> {
    let mut buffer = [0u8; 3];
    match input.read(&mut buffer) {
        Ok(0) => Ok(None),
        Ok(_) => {
            //println!("{:?}", buffer);
            let key = match &buffer {
                [27, 91, 65] => Key::UpArrow,
                [27, 91, 66] => Key::DownArrow,
                [27, 91, 67] => Key::RightArrow,
                [27, 91, 68] => Key::LeftArrow,
                [127, _, _] => Key::Backspace,
                [10, _, _] => Key::Newline,
                [27, _, _] => Key::Escape,
                [19, _, _] => Key::CtrlS,
                [17, _, _] => Key::CtrlQ,
                [byte, _, _] => Key::Char(*byte as char),
            };
            Ok(Some(key))
        }
        Err(ref error) if error.kind() == io::ErrorKind::WouldBlock => {
            thread::sleep(Duration::from_millis(10));
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn get_user_command(editor_state: &mut EditorState, prompt: String) -> String {
    editor_state.menu_info = prompt.clone();
    draw_screen(editor_state);

    let mut command = String::new();

    loop {
        if let Ok(Some(input)) = process_input(&mut editor_state.user_input) {
            match input {
                Key::Escape => return "".to_string(),
                Key::Backspace => {
                    command.pop();
                }
                Key::Newline => return command,
                Key::Char(ch) => {
                    command.push(ch);
                }
                _ => continue,
            }

            editor_state.menu_info = format!("{}{}", prompt, command);
            draw_screen(editor_state);
        }
    }
}

fn handle_key_presses(key: &Key, editor_state: &mut EditorState) -> Result<KeyAction, io::Error> {
    match key {
        Key::UpArrow => move_cursor_up(editor_state),
        Key::DownArrow => move_cursor_down(editor_state),
        Key::LeftArrow => move_cursor_left(editor_state),
        Key::RightArrow => move_cursor_right(editor_state),
        Key::Backspace => delete_character(editor_state),
        Key::Newline => insert_newline(editor_state),
        Key::Escape => {
            Ok(KeyAction::Continue)
        }
        Key::CtrlS => {
            save_file(editor_state)?;
            Ok(KeyAction::Continue)
        }
        Key::CtrlQ => Ok(KeyAction::Exit),
        Key::Char(ch) => insert_character(editor_state, *ch as u8),
    }
}

fn move_cursor_up(editor_state: &mut EditorState) -> Result<KeyAction, io::Error> {
    if editor_state.cursor_y > 0 {
        if editor_state.cursor_x < editor_state.file[editor_state.cursor_y - 1].len() {
            editor_state.cursor_y -= 1;
        }
        else {
            editor_state.cursor_x = editor_state.file[editor_state.cursor_y - 1].len() - 1;
            editor_state.cursor_y -= 1;
        }
    }       
    Ok(KeyAction::Continue)
}

fn move_cursor_down(editor_state: &mut EditorState) -> Result<KeyAction, io::Error> {
    if editor_state.cursor_y + 1 < editor_state.file.len() {
        if editor_state.cursor_x < editor_state.file[editor_state.cursor_y + 1].len() + 1 {
            editor_state.cursor_y += 1;
        } 
        else {
            editor_state.cursor_x = editor_state.file[editor_state.cursor_y + 1].len() - 1;
            editor_state.cursor_y += 1;
        }
    }
    Ok(KeyAction::Continue)
}

fn move_cursor_left(editor_state: &mut EditorState) -> Result<KeyAction, io::Error> {
    if editor_state.cursor_x != 0 {
        editor_state.cursor_x -= 1;
    }
    Ok(KeyAction::Continue)
}

fn move_cursor_right(editor_state: &mut EditorState) -> Result<KeyAction, io::Error> {
    if editor_state.cursor_x + 1 < editor_state.file[editor_state.cursor_y].len() {
        editor_state.cursor_x += 1;
    }
    Ok(KeyAction::Continue)
}

fn delete_character(editor_state: &mut EditorState) -> Result<KeyAction, io::Error> {
    if editor_state.cursor_x > 0 {
        editor_state.file[editor_state.cursor_y].remove(editor_state.cursor_x - 1);
        editor_state.cursor_x -= 1;
    } 
    else if editor_state.cursor_y > 0 {
        editor_state.file[editor_state.cursor_y - 1].pop();

        editor_state.cursor_x = editor_state.file[editor_state.cursor_y - 1].len();

        for b in editor_state.file[editor_state.cursor_y].clone() {
            editor_state.file[editor_state.cursor_y - 1].push(b);
        }

        editor_state.file.remove(editor_state.cursor_y);

        editor_state.cursor_y -= 1;
    }
    Ok(KeyAction::Continue)
}

fn insert_newline(editor_state: &mut EditorState) -> Result<KeyAction, io::Error> {
    let split_vector = editor_state.file[editor_state.cursor_y].split_off(editor_state.cursor_x);
    editor_state.file[editor_state.cursor_y].push('\n' as u8);
    editor_state.file.insert(editor_state.cursor_y + 1, split_vector);
    editor_state.cursor_x = 0;
    editor_state.cursor_y += 1;
    Ok(KeyAction::Continue)
}

fn insert_character(editor_state: &mut EditorState, ch: u8) -> Result<KeyAction, io::Error> {
    editor_state.file[editor_state.cursor_y].insert(editor_state.cursor_x, ch);
    editor_state.cursor_x += 1;
    Ok(KeyAction::Continue)
}


/* GUI & Screen Drawing */
#[repr(C)]
#[derive(Default)]
struct Winsize {
    ws_row: c_ushort,
    ws_col: c_ushort,
    ws_xpixel: c_ushort,
    ws_ypixel: c_ushort,
}

fn draw_screen(editor_state: &mut EditorState) {
    get_winsize(editor_state);
    scroll_screen(editor_state);
    fill_screen_buffer(editor_state);
    clear_screen();
    print!("{}", editor_state.screen_buffer);
    move_cursor_to(editor_state); 
    io::stdout().flush().expect("flush failed.");
}

fn get_winsize(editor_state: &mut EditorState) {
    let mut winsize = Winsize::default();

    unsafe {
        ioctl(STDOUT_FILENO, TIOCGWINSZ, &mut winsize);
    }

    editor_state.terminal_rows = winsize.ws_row as usize;
    editor_state.terminal_cols = winsize.ws_col as usize;
}

fn scroll_screen(editor_state: &mut EditorState) {
    get_render_cursor_x(editor_state);

    if editor_state.cursor_y < editor_state.current_row {
        editor_state.current_row = editor_state.cursor_y;
    }
    if editor_state.cursor_y >= editor_state.current_row + editor_state.terminal_rows.saturating_sub(1) {
        editor_state.current_row = editor_state.cursor_y.saturating_sub(editor_state.terminal_rows.saturating_sub(2));
    }
    if editor_state.rendered_x < editor_state.current_column {
        editor_state.current_column = editor_state.rendered_x;
    }
    if editor_state.cursor_x >= editor_state.current_column + editor_state.terminal_cols {
        editor_state.current_column = editor_state.cursor_x - editor_state.terminal_cols.saturating_sub(1);
    }
}

fn get_render_cursor_x(editor_state: &mut EditorState) {
    editor_state.rendered_x = 0;

    for i in 0..editor_state.cursor_x {
        if editor_state.file[editor_state.cursor_y][i] == b'\t' {
            editor_state.rendered_x += TAB_SPACES - (editor_state.rendered_x % TAB_SPACES);
        }
        else {
            editor_state.rendered_x += 1;
        }
    }
}

fn fill_screen_buffer(editor_state: &mut EditorState) {
    editor_state.screen_buffer.clear();

    for i in editor_state.current_row..(editor_state.current_row + editor_state.terminal_rows - 1) {
        if i < editor_state.file.len() {
            let mut line_string = String::from_utf8_lossy(&editor_state.file[i]).into_owned();

            replace_tabs_with_spaces(&mut line_string);

            if editor_state.current_column < line_string.len() {
                if line_string.len() > editor_state.terminal_cols - 1 {
                    line_string = line_string.chars()
                        .skip(editor_state.current_column)
                        .take(editor_state.terminal_cols - 1)
                        .collect();
                 } 
                 else {
                    line_string = line_string.chars()
                        .skip(editor_state.current_column)
                        .collect();
                 }
            }
            else {
                line_string.clear();
            }

            editor_state.screen_buffer += &line_string.trim_end_matches('\n');
        }
        else {
            editor_state.screen_buffer += "~";
        }
        if i < editor_state.current_row + editor_state.terminal_rows - 1 { editor_state.screen_buffer += "\n"; }
    }
    
    editor_state.screen_buffer += &editor_state.menu_info.clone();
    editor_state.menu_info.clear();
}

fn replace_tabs_with_spaces(line: &mut String) {
    let mut column = 0;
    let mut i = 0;

    while i < line.len() {
        if line.chars().nth(i) == Some('\t') {
            let spaces = TAB_SPACES - (column % TAB_SPACES);
            line.replace_range(i..i+1, &" ".repeat(spaces));
            column += spaces;
            i += spaces;
        } 
        else {
            column += 1;
            i += 1;
        }
    }
}

fn clear_screen() {
    print!("\x1B[2J\x1B[1;1H");
}

fn move_cursor_to(editor_state: &mut EditorState) {
    let move_x = editor_state.rendered_x - editor_state.current_column + 1;
    let move_y = editor_state.cursor_y - editor_state.current_row + 1;

    print!("\x1B[{};{}H", move_y, move_x);
}


/* File Operations */

fn handle_command_line_args(editor_state: &mut EditorState) -> Vec<Vec<u8>> {
    let args: Vec<String> = env::args().collect();

    match args.len() {
        1 => {
            let mut initial_file = Vec::<Vec<u8>>::new();
            initial_file.push(Vec::<u8>::new());
            initial_file
        }
        2 => {
            editor_state.filename = args[1].clone();
            load_file(&args[1])
        }
        _ => Vec::<Vec<u8>>::new(),
    }
}

fn load_file(filename: &String) -> Vec<Vec<u8>> {
    let file = fs::read(filename);
    match file {
        Ok(file) => {
            let mut lines: Vec<Vec<u8>> = Vec::new();
            let mut current_line: Vec<u8> = Vec::new();

            for &byte in &file {
                if byte == b'\n' {
                    current_line.push(byte);
                    lines.push(current_line);
                    current_line = Vec::new();
                } 
                else {
                    current_line.push(byte);
                }
            }
            
            if !current_line.is_empty() {
                lines.push(current_line);
            }

            return lines;
        }       
        Err(error) => {
            println!("{}", error);
            process::exit(0);
        }
    }
}

fn save_file(editor_state: &mut EditorState) -> Result<(), io::Error> {
    let file_content: Vec<u8> = editor_state
        .file
        .iter()
        .flat_map(|line| line.iter().copied())
        .collect();

    if editor_state.filename.is_empty() {
        editor_state.filename = get_user_command(editor_state, "Save As: ".to_string());

        if editor_state.filename.is_empty() {
            return Ok(());
        }
    }

    let write_result = fs::write(&editor_state.filename, &file_content);
    
    match write_result {
        Ok(_) => {
            editor_state.menu_info = format!("Saved file: {}", editor_state.filename);
        }
        Err(_) => {
            editor_state.menu_info = format!("Invalid filename or directory: {}", editor_state.filename);
            editor_state.filename = "".to_string();
        }
    }

    return Ok(());
}


/* Main driver function */

fn main() {
    let mut editor_state = EditorState {
        cursor_x: 0,
        cursor_y: 0,
        current_column: 0,
        current_row: 0,
        rendered_x: 0,
        terminal_cols: 0,
        terminal_rows: 0,
        file: Vec::new(),
        screen_buffer: "".to_string(),
        filename: "".to_string(),
        menu_info: "".to_string(),
        user_input: std::io::stdin().lock(),
    };

    editor_state.file = handle_command_line_args(&mut editor_state);
    let default_termios = setup(&mut editor_state);

    draw_screen(&mut editor_state);

    // main input loop
    loop {
        match process_input(&mut editor_state.user_input) {
            Ok(Some(key)) => {
                //println!("key: {:?}", key);
                
                match handle_key_presses(&key, &mut editor_state) {
                    Ok(action) => match action {
                        KeyAction::Exit => {
                            break;
                        }
                        KeyAction::Continue => {
                            draw_screen(&mut editor_state);
                        }
                    },
                    Err(error) => { println!("{:?}", error); break; }
                }
            }
            Ok(None) => { continue; },
            Err(_) => { break; }
        }
    }

    cleanup(&default_termios);
}
