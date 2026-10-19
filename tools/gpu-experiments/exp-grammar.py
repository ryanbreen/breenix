base = """FRAG
DCL IN[0], POSITION, LINEAR
DCL OUT[0], COLOR
DCL SAMP[0]
DCL SVIEW[0], 2D, FLOAT
DCL CONST[0]
DCL TEMP[0]
IMM[0] INT32 {0, 0, 0, 0}
MOV TEMP[0], IMM[0]
F2I TEMP[0].xy, IN[0]
UADD TEMP[0].xy, TEMP[0], CONST[0]
TXF OUT[0], TEMP[0], SAMP[0], 2D
END
"""
grammar('copy fs baseline 39', base, 39)
grammar('copy fs 38 tokens', base, 38)
grammar('copy fs 100 tokens', base, 100)
grammar('no LINEAR', base.replace(', POSITION, LINEAR', ', POSITION'), 38)
grammar('no SVIEW', base.replace('DCL SVIEW[0], 2D, FLOAT\n', ''), 36)
grammar('IMM without INT32', base.replace('IMM[0] INT32', 'IMM[0]'), 39)
grammar('IMM UINT32', base.replace('IMM[0] INT32', 'IMM[0] UINT32'), 39)
grammar('TXF without target', base.replace(', SAMP[0], 2D', ', SAMP[0]'), 39)
grammar('SVIEW UNORM', base.replace('2D, FLOAT', '2D, UNORM'), 39)
