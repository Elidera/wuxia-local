# Build the app's mascot and icon files from assets/mascot/girl.png:
#   python3 prepare.py
# Writes crates/app/res/girl.rgba, icon_32/64.rgba and assets/icon.png/.ico.
from PIL import Image
girl = Image.open('../../assets/mascot/girl.png').convert('RGBA')
H = 400
W = round(girl.width * H / girl.height)
small = girl.resize((W, H), Image.LANCZOS)
open('../../crates/app/res/girl.rgba', 'wb').write(small.tobytes())
print('remember: GIRL_W/GIRL_H in crates/app/src/mascot/mod.rs =', W, H)
# Icon: square around the face and hood (fractions of the cut picture).
w, h = girl.size
cx, cy, half = 0.615 * w, 0.205 * h, 0.2 * h
face = girl.crop((int(cx - half), int(max(0, cy - half)), int(cx + half), int(cy + half)))
side = max(face.size)
sq = Image.new('RGBA', (side, side), (0, 0, 0, 0))
sq.paste(face, ((side - face.width) // 2, (side - face.height) // 2))
big = sq.resize((256, 256), Image.LANCZOS)
big.save('../../assets/icon.png')
big.save('../../assets/icon.ico', sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
for n in (32, 64):
    open(f'../../crates/app/res/icon_{n}.rgba', 'wb').write(sq.resize((n, n), Image.LANCZOS).tobytes())
print('girl', W, H)
