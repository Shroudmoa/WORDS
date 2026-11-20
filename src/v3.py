import os
import logging
import io
import tempfile
import subprocess
from telegram import Update, InlineKeyboardButton, InlineKeyboardMarkup, ReplyKeyboardMarkup, KeyboardButton
from telegram.constants import ChatAction
from telegram.ext import (
    Application, CommandHandler, MessageHandler, filters, ContextTypes,
    CallbackQueryHandler
)

from gtts import gTTS
from dotenv import load_dotenv

load_dotenv()

logging.basicConfig(
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
    level=logging.INFO
)
logger = logging.getLogger(__name__)

# ==========================
# WHISPER (optional dependency)
# ==========================
# openai-whisper is sdist-only and needs numba/llvmlite, which have no
# musllinux wheels - it frequently cannot be installed (see
# requirements-melisa.txt).  Importing it lazily keeps this module importable
# so the rest of the suite still runs; transcription just reports the problem.
_whisper_model = None


def whisper_available() -> bool:
    """True if openai-whisper is importable and the model is loaded."""
    try:
        import whisper  # noqa: F401
    except Exception:
        return False
    return _whisper_model is not None


def load_whisper():
    """Load the model once, on first use. Returns the model or None."""
    global _whisper_model

    if _whisper_model is not None:
        return _whisper_model

    try:
        import whisper
    except Exception as e:
        logger.error(
            "openai-whisper is not installed - transcription disabled. "
            f"Install it with: pip install -r requirements-melisa.txt ({e})"
        )
        return None

    try:
        _whisper_model = whisper.load_model("base")
        logger.info("Whisper model 'base' loaded")
    except Exception as e:
        logger.error(f"Could not load whisper model: {e}")
        _whisper_model = None

    return _whisper_model

# Languages (compact)
LANGUAGES = {
    "en": "🇬🇧 English",
    "de": "🇩🇪 Deutsch",
    "fa": "🇮🇷 فارسی",
    "es": "🇪🇸 Español",
    "fr": "🇫🇷 Français",
    "it": "🇮🇹 Italiano",
    "pt": "🇵🇹 Português",
    "ru": "🇷🇺 Русский",
}

user_languages = {}

# ==========================
# HELPER FUNCTIONS
# ==========================

def get_user_language(user_id: int) -> str:
    """Get user's language (default: English)."""
    return user_languages.get(user_id, "en")

async def set_user_language(user_id: int, language_code: str):
    """Save user's language preference."""
    user_languages[user_id] = language_code

async def check_channel_membership(application: Application, user_id: int) -> bool:
    """Check if user is member of both channels."""
    try:
        allowed_statuses = ["creator", "administrator", "member", "restricted"]
        for channel in [REQUIRED_CHANNEL_1, REQUIRED_CHANNEL_2]:
            member = await application.bot.get_chat_member(channel, user_id)
            if member.status not in allowed_statuses:
                return False
        return True
    except Exception as e:
        logger.error(f"Channel check error: {e}")
        return False

async def send_subscription_prompt(update: Update, context: ContextTypes.DEFAULT_TYPE):
    """Ask user to join channels."""
    keyboard = [
        [
            InlineKeyboardButton("📱 Channel 1", url=f"https://t.me/{REQUIRED_CHANNEL_1.lstrip('@')}"),
            InlineKeyboardButton("📱 Channel 2", url=f"https://t.me/{REQUIRED_CHANNEL_2.lstrip('@')}")
        ],
        [InlineKeyboardButton("✅ I've Joined!", callback_data="check_membership")]
    ]
    reply_markup = InlineKeyboardMarkup(keyboard)
    
    message = (
        "🔐 **Access Required**\n\n"
        f"Join both channels:\n1️⃣ {REQUIRED_CHANNEL_1}\n2️⃣ {REQUIRED_CHANNEL_2}\n\n"
        "Then click the button below!"
    )

    if update.message:
        await update.message.reply_text(message, reply_markup=reply_markup, parse_mode="Markdown")
    elif update.callback_query:
        await update.callback_query.message.reply_text(message, reply_markup=reply_markup, parse_mode="Markdown")

def convert_to_wav(input_path: str) -> str:
    """Convert audio to WAV 16kHz mono."""
    output_file = tempfile.mktemp(suffix=".wav")
    subprocess.run([
        "ffmpeg", "-i", input_path, "-ac", "1", "-ar", "16000",
        output_file, "-y"
    ], check=True, capture_output=True)
    return output_file

async def voice_to_text(file_path: str, language_code: str) -> str:
    """Convert audio to text with language support."""
    model = load_whisper()

    if model is None:
        return (
            "❌ Transcription is unavailable: openai-whisper is not installed "
            "on this host.\n"
            "Text-to-speech still works."
        )

    try:
        wav_file = convert_to_wav(file_path)
        result = model.transcribe(wav_file, language=language_code)
        os.remove(wav_file)
        return result.get("text", "⚠️ Could not transcribe")
    except Exception as e:
        logger.error(f"Whisper error: {e}")
        return f"❌ Error: {e}"

async def text_to_voice(text: str, language_code: str) -> bytes:
    """Convert text to speech with language support."""
    try:
        tts = gTTS(text=text, lang=language_code)
        with tempfile.NamedTemporaryFile(suffix=".mp3", delete=False) as tmp:
            tts.save(tmp.name)
            tmp_path = tmp.name
        with open(tmp_path, "rb") as f:
            audio_data = f.read()
        os.remove(tmp_path)
        return audio_data
    except Exception as e:
        logger.error(f"gTTS error: {e}")
        return None

# ==========================
# COMMAND HANDLERS
# ==========================

async def start(update: Update, context: ContextTypes.DEFAULT_TYPE):
    """Start command."""
    user_id = update.effective_user.id
    
    if not await check_channel_membership(context.application, user_id):
        await send_subscription_prompt(update, context)
        return
    
    welcome = (
        "🎉 **Welcome!**\n\n"
        "I convert between voice and text.\n\n"
        "🎤 Send voice → Get text\n"
        "📝 Send text → Get voice\n\n"
        "Use /language to change language."
    )
    await update.message.reply_text(welcome, parse_mode="Markdown")

async def help_command(update: Update, context: ContextTypes.DEFAULT_TYPE):
    """Help command."""
    user_id = update.effective_user.id
    
    if not await check_channel_membership(context.application, user_id):
        await send_subscription_prompt(update, context)
        return
    
    lang = get_user_language(user_id)
    help_text = (
        "📚 **Help**\n\n"
        "• Send voice/audio files (mp3/ogg/wav) to transcribe\n"
        "• Maximum file size: 30 MB\n"
        f"• Current language: {LANGUAGES[lang]}\n"
        "• Use /language to change language\n"
        "• Both channels required for access"
    )
    await update.message.reply_text(help_text, parse_mode="Markdown")

async def select_language(update: Update, context: ContextTypes.DEFAULT_TYPE):
    """Language selection."""
    user_id = update.effective_user.id
    
    if not await check_channel_membership(context.application, user_id):
        await send_subscription_prompt(update, context)
        return

    keyboard = [
        [InlineKeyboardButton(name, callback_data=f"lang_{code}") 
         for code, name in list(LANGUAGES.items())[i:i+2]]
        for i in range(0, len(LANGUAGES), 2)
    ]
    
    reply_markup = InlineKeyboardMarkup(keyboard)
    await update.message.reply_text("🌍 **Select your language:**", reply_markup=reply_markup, parse_mode="Markdown")

async def status(update: Update, context: ContextTypes.DEFAULT_TYPE):
    """Status command."""
    user_id = update.effective_user.id
    
    if await check_channel_membership(context.application, user_id):
        await update.message.reply_text("✅ You have access to all features!")
    else:
        await send_subscription_prompt(update, context)

# ==========================
# MESSAGE HANDLERS
# ==========================

async def handle_voice(update: Update, context: ContextTypes.DEFAULT_TYPE):
    """Handle voice/audio files."""
    user_id = update.effective_user.id
    
    if not await check_channel_membership(context.application, user_id):
        await send_subscription_prompt(update, context)
        return

    # 30 MB Limit
    MAX_FILE_SIZE = 30 * 1024 * 1024  # 30 MB in Bytes
    
    try:
        if update.message.voice:
            voice_file = await update.message.voice.get_file()
            file_size = update.message.voice.file_size
        elif update.message.audio:
            voice_file = await update.message.audio.get_file()
            file_size = update.message.audio.file_size
        else:
            await update.message.reply_text("⚠️ Unsupported file type.")
            return

        # Größen-Check
        if file_size and file_size > MAX_FILE_SIZE:
            await update.message.reply_text(
                f"❌ File too large! Maximum size: 30 MB\nYour file: {file_size / (1024*1024):.2f} MB"
            )
            return

        await context.bot.send_chat_action(update.effective_chat.id, ChatAction.TYPING)

        tmp_path = f"/tmp/audio_{user_id}_{update.message.message_id}"
        await voice_file.download_to_drive(tmp_path)

        lang = get_user_language(user_id)
        text = await voice_to_text(tmp_path, lang)
        await update.message.reply_text(f"🎤 **Transcribed Text:**\n\n{text}", parse_mode="Markdown")
        
        os.remove(tmp_path)
    except Exception as e:
        logger.error(f"Voice error: {e}")
        await update.message.reply_text(f"❌ Error: {e}")

async def handle_text(update: Update, context: ContextTypes.DEFAULT_TYPE):
    """Handle text messages."""
    user_id = update.effective_user.id
    
    if not await check_channel_membership(context.application, user_id):
        await send_subscription_prompt(update, context)
        return

    await context.bot.send_chat_action(update.effective_chat.id, ChatAction.UPLOAD_VOICE)
    
    try:
        text = update.message.text
        lang = get_user_language(user_id)
        audio_content = await text_to_voice(text, lang)
        
        if audio_content:
            await update.message.reply_audio(audio=io.BytesIO(audio_content), title="🔊 Generated Voice")
        else:
            await update.message.reply_text("❌ Error generating voice.")
    except Exception as e:
        logger.error(f"Text error: {e}")
        await update.message.reply_text(f"❌ Error: {e}")

# ==========================
# CALLBACK HANDLERS
# ==========================

async def callback_language(update: Update, context: ContextTypes.DEFAULT_TYPE):
    """Handle language selection."""
    query = update.callback_query
    user_id = query.from_user.id
    lang_code = query.data.split("_")[1]
    
    await set_user_language(user_id, lang_code)
    
    await query.answer()
    await query.edit_message_text(f"✅ Language set: {LANGUAGES[lang_code]}")

async def callback_membership(update: Update, context: ContextTypes.DEFAULT_TYPE):
    """Check membership after joining."""
    query = update.callback_query
    user_id = query.from_user.id
    await query.answer()

    if await check_channel_membership(context.application, user_id):
        await query.edit_message_text("✅ **Membership Verified!**\nYou can now use the bot!", parse_mode="Markdown")
    else:
        await query.edit_message_text(
            f"⏳ **Still Pending**\nJoin both channels:\n1️⃣ {REQUIRED_CHANNEL_1}\n2️⃣ {REQUIRED_CHANNEL_2}",
            parse_mode="Markdown"
        )

# ==========================
# MAIN
# ==========================

def build_app():
    app = Application.builder().token(TELEGRAM_TOKEN).build()

    # Commands
    app.add_handler(CommandHandler("start", start))
    app.add_handler(CommandHandler("help", help_command))
    app.add_handler(CommandHandler("language", select_language))
    app.add_handler(CommandHandler("status", status))

    # Messages
    app.add_handler(MessageHandler(filters.VOICE | filters.AUDIO, handle_voice))
    app.add_handler(MessageHandler(filters.TEXT & ~filters.COMMAND, handle_text))

    # Callbacks
    app.add_handler(CallbackQueryHandler(callback_language, pattern="^lang_"))
    app.add_handler(CallbackQueryHandler(callback_membership, pattern="^check_membership$"))

    logger.info("🤖 Bot started!")
    return app

def main():
    build_app().run_polling()

if __name__ == "__main__":
    main()

